mod common;
use common::*;
use kernel::abi::{self, Args, Value};
use runtime::{CpuAbi, Event, ExecCtx, Runner};

#[test]
fn every_declared_export_decodes_its_contract_and_cleans_the_actual_frame() {
    let h = Harness::new();
    let flags = 0x8d7;
    for &export in kernel::EXPORTS {
        let words: Vec<u32> = (0..export.words as u32).map(|i| 0x10203040 + i).collect();
        let expected = words.clone();
        h.runtime.edit_dispatch(|d| {
            abi::register_at(d, CODE, export, move |args, _| {
                for (i, value) in expected.iter().enumerate() {
                    assert_eq!(args.get(i), *value);
                }
                Ok(Value::U64(0x89abcdef_01234567))
            })
        });
        let mut cpu = CpuAbi {
            esp: STACK_TOP - 0x100,
            ebx: 23,
            esi: 24,
            edi: 25,
            ebp: 26,
            eflags: flags,
            ..CpuAbi::xbox_thread()
        };
        frame(
            &h.runtime,
            &mut cpu,
            export,
            &words,
            runtime::exec::HARNESS_EXIT,
        );
        let before = cpu;
        let mut ctx = ExecCtx::new(h.runtime.clone(), 1);
        let transfer = Runner::default().run(&mut cpu, &mut ctx, CODE);
        assert!(
            matches!(ctx.event(transfer.event_id), Some(Event::HarnessExit)),
            "{}",
            export.name
        );
        assert_eq!(
            cpu.esp,
            before.esp + 4 + export.cleanup(),
            "{}",
            export.name
        );
        assert_eq!((cpu.eax, cpu.edx), (0x01234567, 0x89abcdef));
        assert_eq!(
            (cpu.ebx, cpu.esi, cpu.edi, cpu.ebp, cpu.eflags),
            (23, 24, 25, 26, flags)
        );
        assert_eq!(
            (cpu.x87, cpu.xmm, cpu.mxcsr),
            (before.x87, before.xmm, before.mxcsr)
        );
    }
}

#[test]
fn every_export_faults_on_missing_return_or_fixed_stack_word_without_cpu_changes() {
    let h = Harness::new();
    for &export in kernel::EXPORTS {
        for missing_argument in [false, true] {
            if missing_argument && export.stack_words() == 0 {
                continue;
            }
            let esp = if missing_argument {
                STACK_TOP - export.stack_words() as u32 * 4
            } else {
                STACK_TOP
            };
            let mut cpu = CpuAbi {
                esp,
                ..CpuAbi::xbox_thread()
            };
            if missing_argument {
                h.set(esp, runtime::exec::HARNESS_EXIT);
            }
            cpu.eip = CODE;
            let before = cpu;
            let result = Args::read(export, &cpu, &h.runtime.memory());
            assert!(result.is_err(), "{}", export.name);
            assert_eq!(cpu, before);
        }
    }
}

#[test]
fn void_and_word_returns_preserve_uncontracted_registers_and_cdecl_varargs() {
    let h = Harness::new();
    let export = kernel::EXPORTS
        .iter()
        .find(|e| e.ordinal == 8)
        .copied()
        .unwrap();
    let mut cpu = h.cpu;
    cpu.esp -= 16;
    cpu.eax = 111;
    cpu.edx = 222;
    frame(
        &h.runtime,
        &mut cpu,
        export,
        &[DATA, 0xabc, 0xdef],
        CODE + 4,
    );
    let args = Args::read(export, &cpu, &h.runtime.memory()).unwrap();
    assert_eq!(args.vararg(&h.runtime.memory(), 0).unwrap(), 0xabc);
    let before = cpu;
    let transfer = args
        .finish(&mut cpu, &h.runtime.memory(), Value::Void)
        .unwrap();
    assert_eq!(cpu.eax, 111);
    assert_eq!(cpu.edx, 222);
    assert_eq!(cpu.esp, before.esp + 4);
    assert_eq!(transfer.target_pc, CODE + 4);
    cpu = before;
    args.finish(&mut cpu, &h.runtime.memory(), Value::U32(9))
        .unwrap();
    assert_eq!(cpu.edx, 222);
}

#[test]
fn ordinal_inventory_matches_existing_runtime_contracts() {
    let mut ordinals = std::collections::BTreeSet::new();
    for export in kernel::EXPORTS {
        assert!(ordinals.insert(export.ordinal));
        if let Some(import) = runtime::kernel::function(export.ordinal) {
            assert_eq!(export.name, import.name);
            assert_eq!(export.stack_words(), import.stack_words);
            assert_eq!(export.variadic, import.variadic);
        }
    }
}

#[test]
fn a_handler_output_can_change_the_actual_guest_return_word() {
    let mut h = Harness::new();
    h.runtime.edit_dispatch(|dispatch| {
        abi::register(
            dispatch,
            abi::Export::stdcall(15, "return-alias fixture", 2),
            |args, memory| {
                memory.set_u32(args.stack, CODE + 0x100)?;
                Ok(Value::U32(42))
            },
        )
    });
    h.code(0x100, |cpu, ctx, _| {
        assert_eq!(cpu.eax, 42);
        cpu.ebx = 0x1234;
        ctx.memory().ret(cpu, 0)
    });
    h.set(STACK_TOP - 0x100 + 12, runtime::exec::HARNESS_EXIT);
    let transfer = h.call(15, &[DATA, 0]);
    assert!(matches!(
        h.ctx.event(transfer.event_id),
        Some(Event::HarnessExit)
    ));
    assert_eq!(h.cpu.ebx, 0x1234);
    assert_eq!(h.cpu.esp, STACK_TOP - 0x100 + 16);
}

#[test]
fn builtin_service_outputs_can_alias_the_return_slot() {
    let mut h = Harness::config(kernel::Config {
        epoch_100ns: (CODE + 0x100) as u64,
        ..kernel::Config::default()
    });
    let stack = STACK_TOP - 0x100;
    h.set(stack + 8, runtime::exec::HARNESS_EXIT);
    h.code(0x100, |cpu, ctx, _| {
        assert_eq!(cpu.eax, 0xfeedface); // KeQuerySystemTime returns VOID.
        cpu.ebx = 0x5678;
        ctx.memory().ret(cpu, 0)
    });
    let transfer = h.call(128, &[stack]);
    assert!(matches!(
        h.ctx.event(transfer.event_id),
        Some(Event::HarnessExit)
    ));
    assert_eq!(h.cpu.ebx, 0x5678);
    assert_eq!(h.cpu.esp, stack + 12);
}
