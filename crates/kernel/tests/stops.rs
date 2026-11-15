mod common;
use common::*;
use runtime::{CpuAbi, Event, ExecCtx, Runner};

#[test]
fn every_remaining_runtime_import_keeps_a_diagnostic_stop_and_its_frame() {
    let h = Harness::new();
    for import in runtime::kernel::FUNCTION_IMPORTS {
        if kernel::EXPORTS
            .iter()
            .any(|e| e.ordinal == import.ordinal && e.ordinal != 171)
        {
            continue;
        }
        let mut cpu = CpuAbi {
            eip: runtime::kernel::function_token(import.ordinal),
            esp: STACK_TOP - 0x100,
            eax: 0x12345678,
            edx: 0x87654321,
            ..CpuAbi::xbox_thread()
        };
        let mut ctx = ExecCtx::new(h.runtime.clone(), 1);
        h.set(cpu.esp, CODE);
        for i in 0..import.stack_words {
            h.set(cpu.esp + 4 + i as u32 * 4, 100 + i as u32);
        }
        let before = cpu;
        let t = Runner::default().run(&mut cpu, &mut ctx, before.eip);
        assert_eq!(t.kind, runtime::abi::STOP, "{}", import.name);
        let Some(Event::KernelCall(call)) = ctx.event(t.event_id) else {
            panic!("missing kernel stop for {}", import.name);
        };
        assert_eq!(call.ordinal, import.ordinal);
        assert_eq!(call.cpu, before);
        assert_eq!(cpu, before);
        assert_eq!(call.stack_args.len(), import.stack_words);
        assert_eq!(call.return_pc, Ok(CODE));
    }
}

#[test]
fn explicit_stop_policies_cover_unverified_slist_and_io_space() {
    let mut h = Harness::new();
    h.stopped(56, &[DATA]);
    h.stopped(57, &[DATA]);
    h.stopped(58, &[DATA, DATA + 16]);
    h.stopped(177, &[0xfd000000, 4096, 4]);
    let remaining = h.kernel.stopped_ordinals();
    assert!(remaining.contains(&56));
    assert!(remaining.contains(&171));
    assert!(remaining.contains(&177));
    assert!(remaining.contains(&95));
    assert!(!remaining.contains(&255));
    assert_eq!(h.kernel.stop_reasons().len(), 4);
}
