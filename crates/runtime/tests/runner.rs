mod common;
use common::*;
use runtime::{
    CpuAbi, Dispatch, Event, ExecCtxOpaque, GuestFn, Runner, TransferAbi, abi, cpu::ABI_VERSION,
    ffi, kernel, load_xbe,
};
use runtime_test_probe::{h2_test_c_calls_rust, h2_test_c_handler, h2_test_c_memory_widths};

// Every unsafe body in this fixture uses only the audited borrow adapter.
unsafe extern "C" fn entry_to_thunk(
    cpu: *mut CpuAbi,
    ctx: *mut ExecCtxOpaque,
    pc: u32,
) -> TransferAbi {
    // SAFETY: the runner provides this exclusive invocation-scoped pair.
    unsafe {
        common::with_context(cpu, ctx, |cpu, ctx| {
            let result = (|| {
                match pc {
                    ENTRY => ctx.memory().call(cpu, WRAPPER, ENTRY + 5),
                    WRAPPER => {
                        for arg in (0..10u32).rev() {
                            let esp = cpu.esp.wrapping_sub(4);
                            ctx.memory().set_u32(esp, 0xa000 + arg)?;
                            cpu.esp = esp;
                        }
                        // Actual indirect slot load, including later slot rewrites.
                        let token = ctx.memory().u32(THUNK)?;
                        ctx.memory().call(cpu, token, WRAPPER + 6)
                    }
                    _ => Ok(TransferAbi::fault(ctx.emit(Event::InvalidTransfer))),
                }
            })();
            result.unwrap_or_else(|fault| ctx.fault(cpu.eip, fault))
        })
    }
}
fn register(dispatch: &mut Dispatch, pc: u32, f: GuestFn) {
    // SAFETY: fixture handlers use exclusive refs only through with_context.
    unsafe {
        dispatch.register_generated(pc, ABI_VERSION, f).unwrap();
    }
}

#[test]
fn generated_call_to_kernel_thunk_stops_with_all_ten_arguments() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    image.runtime.edit_dispatch(|d| {
        register(d, ENTRY, entry_to_thunk);
        register(d, WRAPPER, entry_to_thunk);
    });
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    let transfer = Runner::default().run(&mut cpu, &mut ctx, ENTRY);
    assert_eq!(transfer.kind, abi::STOP);
    let Event::KernelCall(call) = ctx.event(transfer.event_id).unwrap() else {
        panic!("missing kernel stop");
    };
    assert_eq!(call.ordinal, 255);
    assert_eq!(call.name, "PsCreateSystemThreadEx");
    assert_eq!(call.return_pc, Ok(WRAPPER + 6));
    assert_eq!(
        call.stack_args,
        (0..10).map(|i| Ok(0xa000 + i)).collect::<Vec<_>>()
    );
    assert_eq!(call.cpu, cpu);
    assert_eq!(cpu.eip, kernel::function_token(255));
    assert_eq!(image.cpu.esp - cpu.esp, 48); // two calls plus ten args
}
#[test]
fn slot_rewrite_routes_to_current_guest_target() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    image.runtime.memory().set_u32(THUNK, 0x11040).unwrap();
    image.runtime.edit_dispatch(|d| {
        register(d, ENTRY, entry_to_thunk);
        register(d, WRAPPER, entry_to_thunk);
        let _ = d.register_override(0x11040, "rewritten slot", |cpu, _, _| {
            cpu.eax = 77;
            Ok(TransferAbi::stop(0))
        });
    });
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    assert_eq!(
        Runner::default().run(&mut cpu, &mut ctx, ENTRY).kind,
        abi::STOP
    );
    assert_eq!(cpu.eax, 77);
    assert!(ctx.events().is_empty());
}

unsafe extern "C" fn resumable(cpu: *mut CpuAbi, ctx: *mut ExecCtxOpaque, pc: u32) -> TransferAbi {
    // SAFETY: runner/original resolver provide the exclusive pair.
    unsafe {
        common::with_context(cpu, ctx, |cpu, ctx| {
            let result = match pc {
                ENTRY => {
                    cpu.eax = 1;
                    ctx.memory().call(cpu, WRAPPER, ENTRY + 5)
                }
                WRAPPER => {
                    cpu.eax += 10;
                    ctx.memory().ret(cpu, 0)
                }
                p if p == ENTRY + 5 => {
                    cpu.eax += 1;
                    ctx.memory().ret(cpu, 0)
                }
                _ => Ok(TransferAbi::fault(ctx.emit(Event::InvalidTransfer))),
            };
            result.unwrap_or_else(|fault| ctx.fault(cpu.eip, fault))
        })
    }
}
#[test]
fn resumes_bypass_entry_overrides_original_resolution_is_separate() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    image.runtime.edit_dispatch(|d| {
        register(d, ENTRY, resumable);
        register(d, WRAPPER, resumable);
        d.register_resume(ENTRY + 5, ENTRY, ENTRY + 5).unwrap();
        let _ = d.register_override(ENTRY + 5, "entry at continuation", |cpu, _, _| {
            cpu.eax = 999;
            Ok(TransferAbi::stop(0))
        });
        let _ = d.register_override(ENTRY, "entry override", |cpu, ctx, _| {
            // Exercises table mutation while a resolved override is running.
            ctx.runtime().edit_dispatch(|d| {
                d.set_enabled(WRAPPER, false);
            });
            cpu.eax = 100;
            ctx.memory().call(cpu, WRAPPER, ENTRY + 5)
        });
    });
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    let t = Runner::default().run(&mut cpu, &mut ctx, ENTRY);
    assert_eq!(t.kind, abi::STOP);
    assert_eq!(cpu.eax, 111);
    assert_eq!(ctx.event(t.event_id), Some(&Event::HarnessExit));
    let mut cpu = image.cpu;
    let original = image.runtime.resolve_original(ENTRY).unwrap();
    assert_eq!(original.invoke(&mut cpu, &mut ctx, ENTRY).kind, abi::CALL);
    assert_eq!(cpu.eax, 1);
    assert!(
        image
            .runtime
            .resolve_original(kernel::function_token(255))
            .is_none()
    );
    image.runtime.edit_dispatch(|d| {
        assert!(d.set_enabled(ENTRY, false));
    });
    let mut cpu = image.cpu;
    Runner::default().run(&mut cpu, &mut ctx, ENTRY);
    assert_eq!(cpu.eax, 12);
    let mut cpu = image.cpu;
    Runner::default().run(&mut cpu, &mut ctx, ENTRY + 5);
    assert_eq!(cpu.eax, 999); // explicit entry applies the entry contract
}
#[test]
fn unknown_targets_bad_versions_and_safe_memory_faults_stop() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    image.runtime.edit_dispatch(|d| {
        // SAFETY: the function is valid; the bad version must be rejected.
        assert!(unsafe { d.register_generated(ENTRY, ABI_VERSION + 1, resumable) }.is_err());
        let _ = d.register_override(ENTRY, "unmapped load", |cpu, ctx, _| {
            cpu.eax = ctx.memory().u32(0)?;
            Ok(TransferAbi::stop(0))
        });
    });
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    let t = Runner::default().run(&mut cpu, &mut ctx, ENTRY);
    assert_eq!(t.kind, abi::FAULT);
    assert!(matches!(
        ctx.event(t.event_id),
        Some(Event::MemoryFault { pc: ENTRY, .. })
    ));
    let t = Runner::default().run(&mut cpu, &mut ctx, 0x22222);
    assert!(matches!(
        ctx.event(t.event_id),
        Some(Event::UnsupportedTarget {
            target: 0x22222,
            ..
        })
    ));
    cpu.abi_version = 99;
    let t = Runner::default().run(&mut cpu, &mut ctx, ENTRY);
    assert_eq!(ctx.event(t.event_id), Some(&Event::InvalidAbi));
}

unsafe extern "C" fn rust_called_by_c(
    cpu: *mut CpuAbi,
    _: *mut ExecCtxOpaque,
    pc: u32,
) -> TransferAbi {
    // SAFETY: C forwards its live invocation-scoped CPU unchanged.
    unsafe {
        (*cpu).eax = 0x87654321;
    }
    TransferAbi::call(0x12345678, pc)
}
unsafe extern "C" fn bridge(cpu: *mut CpuAbi, ctx: *mut ExecCtxOpaque, pc: u32) -> TransferAbi {
    // SAFETY: C probe obeys the public header and forwards the exclusive pair.
    unsafe { h2_test_c_calls_rust(rust_called_by_c, cpu, ctx, pc) }
}
#[test]
fn actual_c_handlers_and_rust_callbacks_agree_on_struct_return_abi() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    image.runtime.edit_dispatch(|d| {
        register(d, ENTRY, h2_test_c_handler);
        register(d, WRAPPER, bridge);
    });
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    cpu.esi = THUNK + 0x100;
    let original = image.runtime.resolve_original(ENTRY).unwrap();
    let t = original.invoke(&mut cpu, &mut ctx, ENTRY);
    assert_eq!(t, TransferAbi::call(0x12345678, ENTRY));
    assert_eq!(cpu.eax, 0x12345678);
    assert_eq!(ctx.memory().u32(cpu.esi).unwrap(), 0x12345678);
    assert_eq!(cpu.x87.st[7][9], 0xab);
    assert_eq!(cpu.xmm[7][15], 0xcd);
    let t = image
        .runtime
        .resolve_original(WRAPPER)
        .unwrap()
        .invoke(&mut cpu, &mut ctx, WRAPPER);
    assert_eq!(t, TransferAbi::call(0x12345678, 0xabcdef01));
    assert_eq!(cpu.eax, 0x87654321);
    cpu.esi = 0;
    let t = original.invoke(&mut cpu, &mut ctx, ENTRY);
    assert_eq!(t.kind, abi::FAULT);
    assert!(matches!(
        ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
}

#[test]
fn changed_returns_tail_jumps_and_stdcall_cleanup_use_guest_state() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    image.runtime.edit_dispatch(|d| {
        let _ = d.register_override(ENTRY, "changed return", |cpu, ctx, _| {
            // A guest-controlled return to an entry uses its override.
            ctx.memory().set_u32(cpu.esp, WRAPPER)?;
            ctx.memory().ret(cpu, 40)
        });
        let _ = d.register_override(WRAPPER, "entry after ret", |cpu, _, _| {
            cpu.eax = 37;
            Ok(TransferAbi::jump(0x11040))
        });
        let _ = d.register_override(0x11040, "tail target", |cpu, _, _| {
            cpu.eax += 1;
            Ok(TransferAbi::stop(0))
        });
    });
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    let esp = cpu.esp;
    assert_eq!(
        Runner::default().run(&mut cpu, &mut ctx, ENTRY).kind,
        abi::STOP
    );
    assert_eq!(cpu.esp, esp + 44); // RET consumes guest PC and immediate cleanup
    assert_eq!(cpu.eax, 38);
}

#[test]
fn runner_rejects_invalid_transfers_even_at_its_budget_boundary() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    let runner = Runner { max_transfers: 1 };
    for (transfer, expected) in [
        (
            TransferAbi::new(99, abi::ENTRY, 0, 0, 0),
            Event::InvalidTransfer,
        ),
        (
            TransferAbi::new(abi::STOP, abi::RESUME, 0, 0, 0),
            Event::InvalidTransfer,
        ),
        (TransferAbi::stop(u32::MAX), Event::InvalidTransfer),
        (TransferAbi::jump(ENTRY), Event::TransferLimit),
    ] {
        image.runtime.edit_dispatch(|d| {
            let _ = d.register_override(ENTRY, "invalid fixture", move |_, _, _| Ok(transfer));
        });
        let mut ctx = image.context();
        let mut cpu = image.cpu;
        let t = runner.run(&mut cpu, &mut ctx, ENTRY);
        assert_eq!(ctx.event(t.event_id), Some(&expected));
    }
}

unsafe extern "C" fn ignores_fault(
    cpu: *mut CpuAbi,
    ctx: *mut ExecCtxOpaque,
    pc: u32,
) -> TransferAbi {
    let mut output = [0xab; 16];
    // SAFETY: output is a separate live 16-byte host array and ctx is the
    // runner's exclusive handle. Access faults before the host copy.
    unsafe {
        ffi::h2abi_v1_read(ctx, pc, 0, 16, 1, output.as_mut_ptr());
    }
    let preserved = output == [0xab; 16];
    // SAFETY: CPU is the invocation's exclusive canonical state.
    unsafe {
        (*cpu).eax = u32::from(preserved);
    }
    TransferAbi::stop(0)
}
#[test]
fn native_fault_is_latched_when_handler_discards_helper_status() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    image
        .runtime
        .edit_dispatch(|d| register(d, ENTRY, ignores_fault));
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    let t = Runner::default().run(&mut cpu, &mut ctx, ENTRY);
    assert_eq!(t.kind, abi::FAULT);
    assert_eq!(cpu.eax, 1);
    assert!(matches!(
        ctx.event(t.event_id),
        Some(Event::MemoryFault { pc: ENTRY, .. })
    ));
}

#[test]
fn c_accessors_round_trip_every_width_without_host_pointer_leaks() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    image
        .runtime
        .edit_dispatch(|d| register(d, ENTRY, h2_test_c_memory_widths));
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    cpu.esi = THUNK + 0x100;
    let t = Runner::default().run(&mut cpu, &mut ctx, ENTRY);
    assert_eq!(t.kind, abi::STOP);
    assert!(ctx.events().is_empty());
    for (i, width) in [1, 2, 4, 8, 10, 16].into_iter().enumerate() {
        let mut result = vec![0; width];
        image
            .runtime
            .memory()
            .read_bytes(cpu.esi + i as u32 * 32, &mut result, 1)
            .unwrap();
        assert_eq!(
            result,
            (0..width).map(|n| 0xa0 + n as u8).collect::<Vec<_>>()
        );
    }
}

#[test]
fn a_stopped_guest_call_returns_to_owned_rust_state_after_run_from() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    image.runtime.edit_dispatch(|d| {
        let _ = d.register_override(WRAPPER, "stopped guest callback", |cpu, _, _| {
            cpu.eax = 7;
            Ok(TransferAbi::stop(0))
        });
    });
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    let state = String::from("callback");
    let runtime = std::sync::Arc::clone(ctx.runtime());
    let initial = ctx
        .call_guest(&mut cpu, &mut runtime.memory(), WRAPPER, move |cpu, _| {
            cpu.eax += state.len() as u32;
            Ok(TransferAbi::stop(0))
        })
        .unwrap();
    let runner = Runner::default();
    let stop = runner.run_from(&mut cpu, &mut ctx, initial);
    assert_eq!(stop.kind, abi::STOP);
    assert_eq!(cpu.eax, 7);
    let returning = ctx.memory().ret(&mut cpu, 0).unwrap();
    let done = runner.run_from(&mut cpu, &mut ctx, returning);
    assert_eq!(done.kind, abi::STOP);
    assert_eq!(cpu.eax, 15);
    assert_eq!(cpu.esp, image.cpu.esp);
    assert!(ctx.events().is_empty()); // callback return is not process exit
    let replay = runner.run_from(&mut cpu, &mut ctx, returning);
    assert_eq!(replay.kind, abi::FAULT); // consumed continuation cannot run again
}

#[test]
fn entry_and_return_contracts_at_the_same_pc_are_explicit() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    image.runtime.edit_dispatch(|d| {
        register(d, ENTRY, resumable);
        d.register_resume(ENTRY + 5, ENTRY, ENTRY + 5).unwrap();
        let _ = d.register_override(ENTRY + 5, "external entry", |cpu, _, _| {
            cpu.eax = 999;
            Ok(TransferAbi::stop(0))
        });
    });
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    Runner::default().run_from(&mut cpu, &mut ctx, TransferAbi::ret(ENTRY + 5));
    assert_eq!(cpu.eax, 1);
    let mut cpu = image.cpu;
    Runner::default().run_from(&mut cpu, &mut ctx, TransferAbi::jump(ENTRY + 5));
    assert_eq!(cpu.eax, 999);
}

#[test]
fn only_a_return_to_the_exit_token_ends_the_harness() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    image
        .runtime
        .memory()
        .map_zeroed(0xfffff000, 4096, runtime::guest::Permissions::RX)
        .unwrap();
    image.runtime.edit_dispatch(|d| {
        let _ = d.register_override(
            runtime::exec::HARNESS_EXIT,
            "ordinary entry",
            |cpu, _, _| {
                cpu.eax = 123;
                Ok(TransferAbi::stop(0))
            },
        );
    });
    let mut ctx = image.context();
    for initial in [
        TransferAbi::jump(runtime::exec::HARNESS_EXIT),
        TransferAbi::call(runtime::exec::HARNESS_EXIT, ENTRY),
    ] {
        let mut cpu = image.cpu;
        Runner::default().run_from(&mut cpu, &mut ctx, initial);
        assert_eq!(cpu.eax, 123);
        assert!(ctx.events().is_empty());
    }
    let mut cpu = image.cpu;
    let t = Runner::default().run_from(
        &mut cpu,
        &mut ctx,
        TransferAbi::ret(runtime::exec::HARNESS_EXIT),
    );
    assert_eq!(ctx.event(t.event_id), Some(&Event::HarnessExit));
}

#[test]
fn compiled_object_versions_and_duplicate_originals_are_checked() {
    let mut d = Dispatch::default();
    // SAFETY: probes and handlers come from the test-only C object.
    unsafe {
        assert!(
            d.register_generated_object(
                ENTRY,
                runtime_test_probe::h2gen_bad_abi_version,
                h2_test_c_handler
            )
            .is_err()
        );
        d.register_generated_object(
            ENTRY,
            runtime_test_probe::h2gen_abi_version,
            h2_test_c_handler,
        )
        .unwrap();
        assert!(
            d.register_generated(ENTRY, ABI_VERSION, h2_test_c_handler)
                .is_err()
        );
    }
    assert!(d.register_resume(ENTRY + 5, ENTRY, ENTRY + 5).is_ok());
    assert!(d.register_resume(ENTRY + 5, ENTRY, ENTRY + 5).is_err());
    let _ = d.register_override(ENTRY, "first", |_, _, _| Ok(TransferAbi::stop(0)));
    let previous = d
        .register_override(ENTRY, "second", |_, _, _| Ok(TransferAbi::stop(0)))
        .unwrap();
    assert_eq!(previous.name, "first");
    let removed = d.restore_override(ENTRY, previous).unwrap();
    assert_eq!(removed.name, "second");
    assert_eq!(d.override_name(ENTRY), Some("first"));
}

#[test]
fn data_pages_cannot_be_dispatched_even_with_an_override() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    image.runtime.edit_dispatch(|d| {
        let _ = d.register_override(THUNK, "nonexecutable entry", |cpu, _, _| {
            cpu.eax = 777;
            Ok(TransferAbi::stop(0))
        });
    });
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    let t = Runner::default().run(&mut cpu, &mut ctx, THUNK);
    assert_eq!(t.kind, abi::FAULT);
    assert_eq!(cpu.eax, 0);
    assert!(
        matches!(ctx.event(t.event_id),Some(Event::MemoryFault { fault, .. }) if fault.access == runtime::guest::Access::Execute)
    );
}

#[test]
fn events_are_bounded_and_fault_without_event_is_recorded() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    let t = Runner::default().run_from(&mut cpu, &mut ctx, TransferAbi::fault(0));
    assert!(matches!(
        ctx.event(t.event_id),
        Some(Event::UnrecordedFault { .. })
    ));
    for _ in 0..runtime::events::MAX_EVENTS * 2 {
        ctx.emit(Event::TransferLimit);
    }
    assert_eq!(ctx.events().len(), runtime::events::MAX_EVENTS);
    assert!(ctx.event(t.event_id).is_none()); // oldest record was evicted
    let exit = Runner::default().run_from(
        &mut cpu,
        &mut ctx,
        TransferAbi::ret(runtime::exec::HARNESS_EXIT),
    );
    assert_eq!(exit.kind, abi::STOP);
    assert_eq!(ctx.event(exit.event_id), Some(&Event::HarnessExit));
}

unsafe extern "C" fn fault_then_invalid_abi(
    cpu: *mut CpuAbi,
    ctx: *mut ExecCtxOpaque,
    pc: u32,
) -> TransferAbi {
    // SAFETY: fixture forwards the runner's exclusive pair with a live buffer.
    unsafe {
        let mut byte = 0;
        ffi::h2abi_v1_read(ctx, pc, 0, 1, 1, &mut byte);
        (*cpu).abi_version = 999;
    }
    TransferAbi::stop(0)
}
#[test]
fn an_access_fault_keeps_its_id_when_native_output_has_a_bad_version() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    image
        .runtime
        .edit_dispatch(|d| register(d, ENTRY, fault_then_invalid_abi));
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    let t = Runner::default().run(&mut cpu, &mut ctx, ENTRY);
    assert_eq!(t.event_id, 1);
    assert!(matches!(
        ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
    assert_eq!(ctx.events().len(), 1);
}

unsafe extern "C" fn bad_width(cpu: *mut CpuAbi, ctx: *mut ExecCtxOpaque, pc: u32) -> TransferAbi {
    let mut bytes = [0xaa; 4];
    // SAFETY: live output buffer and invocation context; width deliberately violates the helper contract.
    unsafe {
        ffi::h2abi_v1_read(ctx, pc, THUNK, 3, 1, bytes.as_mut_ptr());
        (*cpu).eax = u32::from_le_bytes(bytes);
    }
    TransferAbi::stop(0)
}
unsafe extern "C" fn null_buffer(_: *mut CpuAbi, ctx: *mut ExecCtxOpaque, pc: u32) -> TransferAbi {
    // SAFETY: null is diagnosed before the output pointer can be dereferenced.
    unsafe {
        ffi::h2abi_v1_read(ctx, pc, THUNK, 4, 1, std::ptr::null_mut());
    }
    TransferAbi::stop(0)
}
#[test]
fn bad_native_access_contracts_record_access_errors() {
    for (f, reason) in [
        (
            bad_width as GuestFn,
            runtime::guest::FaultReason::InvalidWidth,
        ),
        (
            null_buffer as GuestFn,
            runtime::guest::FaultReason::InvalidBuffer,
        ),
    ] {
        let image = load_xbe(&synthetic_xbe()).unwrap();
        image.runtime.edit_dispatch(|d| register(d, ENTRY, f));
        let mut ctx = image.context();
        let mut cpu = image.cpu;
        let t = Runner::default().run(&mut cpu, &mut ctx, ENTRY);
        assert_eq!(t.kind, abi::FAULT);
        assert!(
            matches!(ctx.event(t.event_id),Some(Event::MemoryFault { fault, .. }) if fault.reason==reason)
        );
        if reason == runtime::guest::FaultReason::InvalidWidth {
            assert_eq!(cpu.eax, 0xaaaaaaaa);
        }
    }
}

unsafe extern "C" fn kernel_callback_body(
    cpu: *mut CpuAbi,
    ctx: *mut ExecCtxOpaque,
    pc: u32,
) -> TransferAbi {
    // SAFETY: fixture receives the runner's exclusive invocation pair.
    unsafe {
        common::with_context(cpu, ctx, |cpu, ctx| {
            let result = (|| {
                if pc == WRAPPER {
                    for word in (0..10u32).rev() {
                        let next = cpu.esp.wrapping_sub(4);
                        ctx.memory().set_u32(next, word)?;
                        cpu.esp = next;
                    }
                    let token = ctx.memory().u32(THUNK)?;
                    ctx.memory().call(cpu, token, WRAPPER + 6)
                } else if pc == WRAPPER + 6 {
                    ctx.memory().ret(cpu, 0)
                } else {
                    Ok(TransferAbi::fault(ctx.emit(Event::InvalidTransfer)))
                }
            })();
            result.unwrap_or_else(|fault| ctx.fault(cpu.eip, fault))
        })
    }
}
#[test]
fn rust_callback_survives_kernel_stop_and_generated_resume() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    image.runtime.edit_dispatch(|d| {
        register(d, WRAPPER, kernel_callback_body);
        d.register_resume(WRAPPER + 6, WRAPPER, WRAPPER + 6)
            .unwrap();
        let _ = d.register_override(WRAPPER + 6, "external entry only", |cpu, _, _| {
            cpu.eax = 999;
            Ok(TransferAbi::stop(0))
        });
    });
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    let runtime = std::sync::Arc::clone(ctx.runtime());
    let saved = String::from("resume");
    let call = ctx
        .call_guest(&mut cpu, &mut runtime.memory(), WRAPPER, move |cpu, _| {
            cpu.eax = cpu.eax.wrapping_add(saved.len() as u32);
            Ok(TransferAbi::stop(0))
        })
        .unwrap();
    let runner = Runner::default();
    let stopped = runner.run_from(&mut cpu, &mut ctx, call);
    assert!(
        matches!(ctx.event(stopped.event_id),Some(Event::KernelCall(call)) if call.ordinal==255)
    );
    // Stand in for a completed stdcall kernel handler, preserving its transfer.
    cpu.eax = 12;
    let returned = ctx.memory().ret(&mut cpu, 40).unwrap();
    assert_eq!(returned.target_pc, WRAPPER + 6);
    let done = runner.run_from(&mut cpu, &mut ctx, returned);
    assert_eq!(done.kind, abi::STOP);
    assert_eq!(cpu.eax, 18);
    assert_eq!(cpu.esp, image.cpu.esp);
    assert_eq!(ctx.events().len(), 1);
}

unsafe extern "C" fn fault_with_history_churn(
    cpu: *mut CpuAbi,
    ctx: *mut ExecCtxOpaque,
    pc: u32,
) -> TransferAbi {
    let mut out = 0;
    // SAFETY: fixture forwards its exclusive pair and a live buffer.
    let first = unsafe { ffi::h2abi_v1_read(ctx, pc, 0, 1, 1, &mut out) };
    // SAFETY: no references escaped the helper; this is the same exclusive pair.
    unsafe {
        common::with_context(cpu, ctx, |cpu, ctx| {
            for _ in 0..runtime::events::MAX_EVENTS * 2 {
                ctx.emit(Event::TransferLimit);
            }
            let taken = ctx.take_events();
            cpu.eax = u32::from(ctx.event(first).is_some());
            cpu.ebx = taken.len() as u32;
        });
    }
    TransferAbi::stop(0)
}
#[test]
fn an_unreported_fault_survives_history_eviction_and_take() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    image
        .runtime
        .edit_dispatch(|d| register(d, ENTRY, fault_with_history_churn));
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    let t = Runner::default().run(&mut cpu, &mut ctx, ENTRY);
    assert_eq!(t.kind, abi::FAULT);
    assert_eq!(cpu.eax, 1);
    assert_eq!(cpu.ebx as usize, runtime::events::MAX_EVENTS - 1);
    assert!(matches!(
        ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
    assert_eq!(ctx.take_events().len(), 1);
    assert!(ctx.events().is_empty());
}

unsafe extern "C" fn secondary_faults_are_diagnostics(
    cpu: *mut CpuAbi,
    ctx: *mut ExecCtxOpaque,
    pc: u32,
) -> TransferAbi {
    let mut byte = 0;
    // SAFETY: fixture forwards the current pair and valid host output storage.
    let first = unsafe { ffi::h2abi_v1_read(ctx, pc, 0, 1, 1, &mut byte) };
    // Intentionally broken generated code: the second access must not replace
    // the fault from the instruction at which execution should have stopped.
    let second = unsafe { ffi::h2abi_v1_read(ctx, pc + 1, 4, 1, 1, &mut byte) };
    // SAFETY: no references escaped the helpers; this remains the exclusive pair.
    unsafe {
        common::with_context(cpu, ctx, |cpu, ctx| {
            for _ in 0..runtime::events::MAX_EVENTS * 2 {
                ctx.emit(Event::TransferLimit);
            }
            cpu.eax = u32::from(ctx.event(first).is_some());
            cpu.ebx = u32::from(ctx.event(second).is_none());
            cpu.esi = first;
        });
    }
    TransferAbi::fault(second)
}
#[test]
fn the_first_ffi_fault_wins_and_later_faults_may_be_evicted() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    image
        .runtime
        .edit_dispatch(|d| register(d, ENTRY, secondary_faults_are_diagnostics));
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    let outcome = Runner::default().run(&mut cpu, &mut ctx, ENTRY);
    assert_eq!(outcome.kind, abi::FAULT);
    assert_eq!(outcome.event_id, cpu.esi);
    assert_eq!((cpu.eax, cpu.ebx), (1, 1));
    assert!(matches!(
        ctx.event(outcome.event_id),
        Some(Event::MemoryFault { pc: ENTRY, .. })
    ));
}

#[test]
fn editing_dispatch_reaches_a_context_that_already_resolved_the_target() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    let returning = |eax: u32| {
        move |cpu: &mut CpuAbi, _: &mut runtime::ExecCtx, _| {
            cpu.eax = eax;
            Ok(TransferAbi::stop(0))
        }
    };
    image.runtime.edit_dispatch(|d| {
        let _ = d.register_override(ENTRY, "first", returning(1));
    });
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    for _ in 0..2 {
        Runner::default().run(&mut cpu, &mut ctx, ENTRY);
        assert_eq!(cpu.eax, 1);
    }
    // Same context, same target: the cached handler must not survive the edit.
    image.runtime.edit_dispatch(|d| {
        let _ = d.register_override(ENTRY, "second", returning(2));
    });
    Runner::default().run(&mut cpu, &mut ctx, ENTRY);
    assert_eq!(cpu.eax, 2);
}

/// Stops at `WRAPPER + 3` as a generated handler does on an instruction it
/// cannot run.
unsafe extern "C" fn stops_midway(
    cpu: *mut CpuAbi,
    ctx: *mut ExecCtxOpaque,
    _: u32,
) -> TransferAbi {
    // SAFETY: the runner provides this exclusive invocation-scoped pair.
    unsafe {
        common::with_context(cpu, ctx, |cpu, ctx| {
            cpu.eip = WRAPPER + 3;
            cpu.ebx = 0x5150;
            TransferAbi::stop(ctx.emit(Event::UnrecordedFault { pc: WRAPPER + 3 }))
        })
    }
}

#[test]
fn stop_recovery_sees_generated_stops_only_and_its_transfer_runs() {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};
    let image = load_xbe(&synthetic_xbe()).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(AtomicU32::new(0));
    let (s, c) = (Arc::clone(&seen), Arc::clone(&calls));
    image.runtime.edit_dispatch(|d| {
        register(d, ENTRY, stops_midway);
        let _ = d.register_override(WRAPPER, "rust stop", |cpu, _, _| {
            cpu.eax = 7;
            Ok(TransferAbi::stop(0))
        });
        d.set_stop_recovery(
            move |cpu: &mut CpuAbi, _: &mut runtime::ExecCtx, t: TransferAbi| {
                c.fetch_add(1, Ordering::SeqCst);
                s.lock().unwrap().push((cpu.eip, cpu.ebx, t.kind));
                // Recover the way the boot's interpreter would: carry on.
                TransferAbi::jump(WRAPPER)
            },
        );
    });
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    let t = Runner::default().run(&mut cpu, &mut ctx, ENTRY);
    // The handler's STOP was recovered with the CPU it left; the override's
    // own STOP was not offered and ends the run.
    assert_eq!(*seen.lock().unwrap(), [(WRAPPER + 3, 0x5150, abi::STOP)]);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!((t.kind, t.event_id, cpu.eax), (abi::STOP, 0, 7));

    // Without the hook the generated STOP ends the run where it happened.
    image.runtime.edit_dispatch(|d| d.clear_stop_recovery());
    let mut cpu = image.cpu;
    let t = Runner::default().run(&mut cpu, &mut ctx, ENTRY);
    assert_eq!(t.kind, abi::STOP);
    assert_eq!(
        ctx.event(t.event_id),
        Some(&Event::UnrecordedFault { pc: WRAPPER + 3 })
    );
    assert_eq!(cpu.eip, WRAPPER + 3);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

/// What a fixture's runs leave behind: every run's transfer, the final CPU,
/// every event and one guest word a callback wrote.
type Outcome = (Vec<TransferAbi>, CpuAbi, Vec<(u32, Event)>, u32);
const MARK: u32 = THUNK + 0x800;

fn outcome(ts: Vec<TransferAbi>, cpu: CpuAbi, ctx: &mut runtime::ExecCtx) -> Outcome {
    let mark = ctx.memory().u32(MARK).unwrap();
    (ts, cpu, ctx.take_events(), mark)
}

/// The lean runner (`DIAG` off, as a release build without stats takes)
/// gives the same transfers, events and final CPU as the diagnostic one, on
/// this file's fixtures plus a preemption and an interrupt.
#[test]
fn lean_and_diag_runners_agree() {
    let fixtures: [(&str, fn(bool) -> Outcome); 9] = [
        ("kernel thunk", |diag| {
            let image = load_xbe(&synthetic_xbe()).unwrap();
            image.runtime.edit_dispatch(|d| {
                register(d, ENTRY, entry_to_thunk);
                register(d, WRAPPER, entry_to_thunk);
            });
            let (mut ctx, mut cpu) = (image.context(), image.cpu);
            let t = Runner::default().run_from_instance(&mut cpu, &mut ctx, TransferAbi::jump(ENTRY), diag);
            outcome(vec![t], cpu, &mut ctx)
        }),
        ("resumes and overrides", |diag| {
            let image = load_xbe(&synthetic_xbe()).unwrap();
            image.runtime.edit_dispatch(|d| {
                register(d, ENTRY, resumable);
                register(d, WRAPPER, resumable);
                d.register_resume(ENTRY + 5, ENTRY, ENTRY + 5).unwrap();
                let _ = d.register_override(ENTRY + 5, "entry at continuation", |cpu, _, _| {
                    cpu.eax = 999;
                    Ok(TransferAbi::stop(0))
                });
            });
            let (mut ctx, mut cpu) = (image.context(), image.cpu);
            let runner = Runner::default();
            let mut ts = vec![runner.run_from_instance(&mut cpu, &mut ctx, TransferAbi::jump(ENTRY), diag)];
            ts.push(runner.run_from_instance(&mut cpu, &mut ctx, TransferAbi::ret(ENTRY + 5), diag));
            ts.push(runner.run_from_instance(&mut cpu, &mut ctx, TransferAbi::jump(ENTRY + 5), diag));
            ts.push(runner.run_from_instance(&mut cpu, &mut ctx, TransferAbi::jump(0x22222), diag));
            outcome(ts, cpu, &mut ctx)
        }),
        ("changed returns and tail jumps", |diag| {
            let image = load_xbe(&synthetic_xbe()).unwrap();
            image.runtime.edit_dispatch(|d| {
                let _ = d.register_override(ENTRY, "changed return", |cpu, ctx, _| {
                    ctx.memory().set_u32(cpu.esp, WRAPPER)?;
                    ctx.memory().ret(cpu, 40)
                });
                let _ = d.register_override(WRAPPER, "entry after ret", |cpu, _, _| {
                    cpu.eax = 37;
                    Ok(TransferAbi::jump(0x11040))
                });
                let _ = d.register_override(0x11040, "tail target", |cpu, _, _| {
                    cpu.eax += 1;
                    Ok(TransferAbi::stop(0))
                });
            });
            let (mut ctx, mut cpu) = (image.context(), image.cpu);
            let t = Runner::default().run_from_instance(&mut cpu, &mut ctx, TransferAbi::jump(ENTRY), diag);
            outcome(vec![t], cpu, &mut ctx)
        }),
        ("transfer limit", |diag| {
            let image = load_xbe(&synthetic_xbe()).unwrap();
            image.runtime.edit_dispatch(|d| {
                let _ = d.register_override(ENTRY, "spin", |cpu, _, _| {
                    cpu.ebx += 1;
                    Ok(TransferAbi::jump(ENTRY))
                });
            });
            let (mut ctx, mut cpu) = (image.context(), image.cpu);
            let t = Runner { max_transfers: 7 }.run_from_instance(&mut cpu, &mut ctx, TransferAbi::jump(ENTRY), diag);
            outcome(vec![t], cpu, &mut ctx)
        }),
        ("latched native fault", |diag| {
            let image = load_xbe(&synthetic_xbe()).unwrap();
            image.runtime.edit_dispatch(|d| register(d, ENTRY, ignores_fault));
            let (mut ctx, mut cpu) = (image.context(), image.cpu);
            let t = Runner::default().run_from_instance(&mut cpu, &mut ctx, TransferAbi::jump(ENTRY), diag);
            outcome(vec![t], cpu, &mut ctx)
        }),
        ("guest call stopped and resumed", |diag| {
            let image = load_xbe(&synthetic_xbe()).unwrap();
            image.runtime.edit_dispatch(|d| {
                let _ = d.register_override(WRAPPER, "stopped guest callback", |cpu, _, _| {
                    cpu.eax = 7;
                    Ok(TransferAbi::stop(0))
                });
            });
            let (mut ctx, mut cpu) = (image.context(), image.cpu);
            let runtime = std::sync::Arc::clone(ctx.runtime());
            let initial = ctx
                .call_guest(&mut cpu, &mut runtime.memory(), WRAPPER, |cpu, _| {
                    cpu.eax += 8;
                    Ok(TransferAbi::stop(0))
                })
                .unwrap();
            let runner = Runner::default();
            let mut ts = vec![runner.run_from_instance(&mut cpu, &mut ctx, initial, diag)];
            let returning = ctx.memory().ret(&mut cpu, 0).unwrap();
            ts.push(runner.run_from_instance(&mut cpu, &mut ctx, returning, diag));
            ts.push(runner.run_from_instance(&mut cpu, &mut ctx, returning, diag));
            outcome(ts, cpu, &mut ctx)
        }),
        ("stop recovery", |diag| {
            let image = load_xbe(&synthetic_xbe()).unwrap();
            image.runtime.edit_dispatch(|d| {
                register(d, ENTRY, stops_midway);
                let _ = d.register_override(WRAPPER, "rust stop", |cpu, _, _| {
                    cpu.eax = 7;
                    Ok(TransferAbi::stop(0))
                });
                d.set_stop_recovery(|_: &mut CpuAbi, _: &mut runtime::ExecCtx, _: TransferAbi| {
                    TransferAbi::jump(WRAPPER)
                });
            });
            let (mut ctx, mut cpu) = (image.context(), image.cpu);
            let t = Runner::default().run_from_instance(&mut cpu, &mut ctx, TransferAbi::jump(ENTRY), diag);
            outcome(vec![t], cpu, &mut ctx)
        }),
        ("slice and request preemption", |diag| {
            let image = load_xbe(&synthetic_xbe()).unwrap();
            image.runtime.edit_dispatch(|d| {
                let _ = d.register_override(ENTRY, "charge", |cpu, ctx, _| {
                    cpu.ebx += 1;
                    ctx.charge_accesses(1);
                    Ok(TransferAbi::jump(ENTRY))
                });
            });
            let (mut ctx, mut cpu) = (image.context(), image.cpu);
            let runner = Runner::default();
            ctx.arm_preemption(3);
            let mut ts = vec![runner.run_from_instance(&mut cpu, &mut ctx, TransferAbi::jump(ENTRY), diag)];
            let resume = ctx.take_preempted().unwrap();
            ctx.arm_preemption(0);
            image.runtime.request_preempt(runtime::preempt::REQUEST);
            ts.push(runner.run_from_instance(&mut cpu, &mut ctx, resume, diag));
            ts.push(ctx.take_preempted().unwrap());
            outcome(ts, cpu, &mut ctx)
        }),
        ("interrupt delivered at a safe point", |diag| {
            let image = load_xbe(&synthetic_xbe()).unwrap();
            image.runtime.edit_dispatch(|d| {
                let _ = d.register_override(ENTRY, "loop", |cpu, _, _| {
                    cpu.ebx += 1;
                    Ok(if cpu.ebx == 4 { TransferAbi::stop(0) } else { TransferAbi::jump(ENTRY) })
                });
                let _ = d.register_override(WRAPPER, "callback", |cpu, ctx, _| {
                    let arg = ctx.memory().u32(cpu.esp.wrapping_add(4))?;
                    ctx.memory().set_u32(MARK, arg + 1)?;
                    ctx.memory().ret(cpu, 4)
                });
            });
            let raised = image.runtime.raise_interrupt(runtime::GuestInterrupt {
                target: WRAPPER,
                args: vec![0x41],
                convention: runtime::CallConvention::Stdcall,
                thread: None,
                coalesce: false,
                name: "fixture",
            });
            assert_eq!(raised, runtime::Raise::Queued);
            let (mut ctx, mut cpu) = (image.context(), image.cpu);
            let t = Runner::default().run_from_instance(&mut cpu, &mut ctx, TransferAbi::jump(ENTRY), diag);
            outcome(vec![t], cpu, &mut ctx)
        }),
    ];
    for (name, fixture) in fixtures {
        let diag = fixture(true);
        assert!(!diag.0.is_empty(), "{name}");
        assert_eq!(fixture(false), diag, "{name}: lean against diagnostic");
    }
    // The interrupt really ran, and the preemption really stopped.
    assert_eq!((fixtures[8].1)(false).3, 0x42);
    assert_eq!((fixtures[7].1)(false).0[0], TransferAbi::stop(0));
}

/// Both instances refuse a CPU whose ABI header is broken when the run
/// starts; the lean one checks it there only, not after every handler.
#[test]
fn lean_and_diag_runners_refuse_a_broken_cpu_header() {
    for diag in [true, false] {
        let image = load_xbe(&synthetic_xbe()).unwrap();
        image.runtime.edit_dispatch(|d| {
            let _ = d.register_override(ENTRY, "never runs", |cpu, _, _| {
                cpu.eax = 1;
                Ok(TransferAbi::stop(0))
            });
        });
        let (mut ctx, mut cpu) = (image.context(), image.cpu);
        cpu.abi_version = 99;
        let t = Runner::default().run_from_instance(&mut cpu, &mut ctx, TransferAbi::jump(ENTRY), diag);
        assert_eq!(ctx.event(t.event_id), Some(&Event::InvalidAbi), "diag={diag}");
        assert_eq!(cpu.eax, image.cpu.eax, "diag={diag}: no handler ran");
    }
}
