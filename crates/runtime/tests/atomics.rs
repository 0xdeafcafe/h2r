use runtime::{
    CpuAbi, Dispatch, ExecCtx, Guest, Runner, Runtime, abi,
    atomic::AtomicOp,
    guest::{FaultReason, Permissions},
};
use std::sync::Arc;

fn memory() -> Guest {
    let mut m = Guest::new();
    m.map(0x1000, 4096, 0, Permissions::RW).unwrap();
    m.map(0x2000, 4096, 8192, Permissions::RW).unwrap();
    m.map(0x80001000, 4096, 0, Permissions::RW).unwrap();
    m
}
#[test]
fn locked_flags_widths_and_accumulator_slices() {
    for width in [1, 2, 4] {
        let mut m = memory();
        let mut cpu = CpuAbi::default();
        let sign = 1u32 << (width * 8 - 1);
        m.set_u32(0x1000, sign - 1).unwrap();
        assert_eq!(
            m.atomic_rmw(&mut cpu, 0x1000, width, AtomicOp::Add, 1)
                .unwrap(),
            sign - 1
        );
        assert_eq!(cpu.eflags & 0x8d5, if width == 1 { 0x890 } else { 0x894 }); // OF SF AF PF
        cpu.eflags |= 1;
        m.atomic_rmw(&mut cpu, 0x1000, width, AtomicOp::Inc, 0)
            .unwrap();
        assert_eq!(cpu.eflags & 1, 1);
        let before = cpu.eflags;
        m.atomic_rmw(&mut cpu, 0x1000, width, AtomicOp::Not, 0)
            .unwrap();
        assert_eq!(cpu.eflags, before);
        m.atomic_xchg(0x1000, width, 42).unwrap();
        assert_eq!(cpu.eflags, before);
        cpu.eax = 0xabcdef00 | 41;
        let mask = u32::MAX >> (32 - width * 8);
        let prior = cpu.eax;
        assert_eq!(m.atomic_cmpxchg(&mut cpu, 0x1000, width, 99).unwrap(), 42);
        assert_eq!(cpu.eax, (prior & !mask) | 42);
        assert_eq!(cpu.eflags & 0x40, 0);
        m.atomic_cmpxchg(&mut cpu, 0x1000, width, 99).unwrap();
        assert_eq!(m.u8(0x1000).unwrap(), 99);
        assert_eq!(cpu.eflags & 0x40, 0x40);
    }
}
#[test]
fn cmpxchg8b_and_faults_preserve_the_required_state() {
    let mut m = memory();
    let mut cpu = CpuAbi::default();
    m.set_u64(0x1ffc, 0x123456789abcdef0).unwrap();
    cpu.eax = 0;
    cpu.edx = 0;
    cpu.eflags = 0x803;
    m.atomic_cmpxchg8b(&mut cpu, 0x1ffc).unwrap();
    assert_eq!((cpu.edx, cpu.eax), (0x12345678, 0x9abcdef0));
    assert_eq!(cpu.eflags, 0x803);
    cpu.ebx = 0x11111111;
    cpu.ecx = 0x22222222;
    m.atomic_cmpxchg8b(&mut cpu, 0x1ffc).unwrap();
    assert_eq!(m.u64(0x1ffc).unwrap(), 0x2222222211111111);
    assert_eq!(cpu.eflags, 0x843);
    m.protect(0x2000, Permissions::READ).unwrap();
    let before = cpu;
    assert_eq!(
        m.atomic_rmw(&mut cpu, 0x1ffe, 4, AtomicOp::Add, 1)
            .unwrap_err()
            .reason,
        FaultReason::Protection
    );
    assert_eq!(cpu, before);
    assert_eq!(m.u64(0x1ffc).unwrap(), 0x2222222211111111);
    assert!(m.atomic_cmpxchg8b(&mut cpu, 0x1ffc).is_err());
    assert_eq!(cpu, before);
}
#[test]
fn aliased_atomic_updates_do_not_lose_writes_between_host_threads() {
    let runtime = Runtime::new(memory(), Dispatch::default());
    let threads: Vec<_> = (0..4)
        .map(|i| {
            let runtime = Arc::clone(&runtime);
            std::thread::spawn(move || {
                let mut cpu = CpuAbi::default();
                for _ in 0..500 {
                    runtime
                        .memory()
                        .atomic_rmw(
                            &mut cpu,
                            if i % 2 == 0 { 0x1000 } else { 0x80001000 },
                            4,
                            AtomicOp::Add,
                            1,
                        )
                        .unwrap();
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(runtime.memory().u32(0x1000).unwrap(), 2000);
}
#[test]
fn c_atomic_entry_points_share_the_rust_memory_contract() {
    let mut m = memory();
    m.map_zeroed(0x3000, 4096, Permissions::RX).unwrap();
    let mut d = Dispatch::default();
    // SAFETY: the C probe was compiled from h2abi.h and reports its own version.
    unsafe {
        d.register_generated_object(
            0x3000,
            runtime_test_probe::h2gen_abi_version,
            runtime_test_probe::h2_test_c_atomics,
        )
        .unwrap();
    }
    let runtime = Runtime::new(m, d);
    let mut ctx = ExecCtx::new(runtime, 1);
    let mut cpu = CpuAbi {
        esi: 0x1000,
        ..CpuAbi::default()
    };
    assert_eq!(
        Runner::default().run(&mut cpu, &mut ctx, 0x3000).kind,
        abi::STOP
    );
    assert_eq!(ctx.memory().u64(0x1000).unwrap(), 44);
}

#[test]
fn arithmetic_carry_logic_and_bit_operations_have_explicit_flags() {
    let mut m = memory();
    let mut cpu = CpuAbi::default();
    for (op, old, operand, carry, result, flags) in [
        (AtomicOp::Adc, 0xff, 0, 1, 0, 0x55),
        (AtomicOp::Sbb, 0, 0, 1, 0xff, 0x95),
        (AtomicOp::Sub, 0x80, 1, 0, 0x7f, 0x810),
        (AtomicOp::Neg, 0x80, 0, 0, 0x80, 0x881),
        (AtomicOp::Dec, 0x80, 0, 1, 0x7f, 0x811),
        (AtomicOp::And, 0xf0, 0x0f, 0, 0, 0x44),
        (AtomicOp::Or, 0x80, 1, 0, 0x81, 0x84),
        (AtomicOp::Xor, 0xff, 0x80, 0, 0x7f, 0),
    ] {
        m.set_u8(0x1000, old).unwrap();
        cpu.eflags = 2 | carry;
        cpu.eflags_defined = u32::MAX;
        m.atomic_rmw(&mut cpu, 0x1000, 1, op, operand).unwrap();
        assert_eq!(m.u8(0x1000).unwrap(), result, "{op:?}");
        assert_eq!(cpu.eflags & 0x8d5, flags, "{op:?}");
    }
    m.set_u32(0x1000, 1).unwrap();
    cpu.eflags = 0x42;
    m.atomic_rmw(&mut cpu, 0x1000, 4, AtomicOp::Bts, 0).unwrap();
    assert_eq!(cpu.eflags & 0x41, 0x41);
    m.atomic_rmw(&mut cpu, 0x1000, 4, AtomicOp::Btr, 0).unwrap();
    assert_eq!(m.u32(0x1000).unwrap(), 0);
    m.atomic_rmw(&mut cpu, 0x1000, 4, AtomicOp::Btc, 31)
        .unwrap();
    assert_eq!(m.u32(0x1000).unwrap(), 0x80000000);
}

#[test]
fn c_atomic_outputs_may_alias_cpu_fields_and_stay_unchanged_on_fault() {
    let mut m = memory();
    m.map_zeroed(0x3000, 4096, Permissions::RX).unwrap();
    m.set_u32(0x1000, 44).unwrap();
    let mut d = Dispatch::default();
    // SAFETY: the C fixture follows h2abi.h, including the CPU output alias rule.
    unsafe {
        d.register_generated_object(
            0x3000,
            runtime_test_probe::h2gen_abi_version,
            runtime_test_probe::h2_test_c_atomic_aliases,
        )
        .unwrap();
    }
    let mut ctx = ExecCtx::new(Runtime::new(m, d), 1);
    let mut cpu = CpuAbi {
        esi: 0x1000,
        ..CpuAbi::default()
    };
    let t = Runner::default().run(&mut cpu, &mut ctx, 0x3000);
    assert_eq!(t.kind, abi::FAULT);
    assert!(
        matches!(ctx.event(t.event_id),Some(runtime::Event::MemoryFault { fault, .. }) if fault.addr==0)
    );
    assert_eq!(cpu.eax, 0x12345678);
    assert_eq!(cpu.ebx, 9);
    assert_eq!(ctx.memory().u32(0x1000).unwrap(), 11);
    assert_eq!(
        runtime::ffi::runtime_abi_version().get(),
        runtime::cpu::ABI_VERSION
    );
    assert_eq!(runtime::ffi::h2abi_v1_version(), runtime::cpu::ABI_VERSION);
}
