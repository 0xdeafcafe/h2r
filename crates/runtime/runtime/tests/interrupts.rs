use runtime::guest::Permissions;
use runtime::{
    CallConvention, CpuAbi, Dispatch, Event, ExecCtx, Guest, GuestInterrupt, Raise, Runner,
    Runtime, TransferAbi, abi, exec::HARNESS_EXIT, interrupt::MAX_PENDING,
};
use std::sync::Arc;

const CODE: u32 = 0x10000;
const MAIN: u32 = CODE;
const ISR: u32 = CODE + 0x100;
const ISR_STD: u32 = CODE + 0x200;
const ISR_BAD: u32 = CODE + 0x300;
const ISR_RAISE: u32 = CODE + 0x400;
const ISR_TAIL: u32 = CODE + 0x500;
const STACK: u32 = 0x20000;
const STACK_TOP: u32 = STACK + 0x4000;
/// Main's loop counter, then its limit.
const COUNT: u32 = 0x30000;
const LIMIT: u32 = 0x30004;
/// Callback log: number of entries, then (target, arg0, DF on entry, main
/// count at entry) per entry.
const LOG: u32 = 0x30100;
const DF: u32 = 1 << 10;

fn log(cpu: &CpuAbi, ctx: &mut ExecCtx, target: u32) -> Result<(), runtime::GuestFault> {
    let mut m = ctx.memory();
    let n = m.u32(LOG)?;
    let at = LOG + 4 + n * 16;
    m.set_u32(at, target)?;
    let arg = m.u32(cpu.esp + 4)?;
    m.set_u32(at + 4, arg)?;
    m.set_u32(at + 8, cpu.eflags & DF)?;
    let count = m.u32(COUNT)?;
    m.set_u32(at + 12, count)?;
    m.set_u32(LOG, n + 1)
}
fn entries(rt: &Runtime) -> Vec<[u32; 4]> {
    let m = rt.memory();
    let n = m.u32(LOG).unwrap();
    (0..n)
        .map(|i| std::array::from_fn(|k| m.u32(LOG + 4 + i * 16 + k as u32 * 4).unwrap()))
        .collect()
}

/// MAIN jumps to itself until COUNT reaches LIMIT, then returns. Each
/// callback logs itself and clobbers every GPR it can.
fn setup(limit: u32) -> (Arc<Runtime>, CpuAbi) {
    let mut g = Guest::new();
    g.map_zeroed(CODE, 0x1000, Permissions::RX).unwrap();
    g.map_zeroed(STACK, STACK_TOP - STACK, Permissions::RW)
        .unwrap();
    g.map_zeroed(COUNT, 0x1000, Permissions::RW).unwrap();
    g.set_u32(LIMIT, limit).unwrap();
    g.set_u32(STACK_TOP - 4, HARNESS_EXIT).unwrap();
    let mut d = Dispatch::default();
    let _ = d.register_override(MAIN, "main", |cpu, ctx, _| {
        let mut m = ctx.memory();
        let n = m.u32(COUNT)?;
        if n >= m.u32(LIMIT)? {
            return m.ret(cpu, 0);
        }
        m.set_u32(COUNT, n + 1)?;
        Ok(TransferAbi::jump(MAIN))
    });
    fn clobber(cpu: &mut CpuAbi) {
        (cpu.eax, cpu.ecx, cpu.edx, cpu.ebx) = (0xdead_0001, 2, 3, 4);
        (cpu.ebp, cpu.esi, cpu.edi) = (5, 6, 7);
        cpu.eflags |= 0x8d5;
        cpu.xmm[0] = [0xee; 16];
        cpu.x87.st[3] = [0x5a; 10];
        (cpu.x87.control, cpu.x87.status, cpu.x87.tag) = (0x0c7f, 0x3800, 0);
        cpu.x87.data_pointer = 0x1234;
        cpu.mxcsr = 0x9fc0;
    }
    let _ = d.register_override(ISR, "isr", |cpu, ctx, _| {
        log(cpu, ctx, ISR)?;
        clobber(cpu);
        ctx.memory().ret(cpu, 0)
    });
    let _ = d.register_override(ISR_STD, "isr stdcall", |cpu, ctx, _| {
        log(cpu, ctx, ISR_STD)?;
        clobber(cpu);
        ctx.memory().ret(cpu, 8)
    });
    let _ = d.register_override(ISR_BAD, "isr bad", |cpu, ctx, _| {
        log(cpu, ctx, ISR_BAD)?;
        ctx.memory().ret(cpu, 4)
    });
    let _ = d.register_override(ISR_RAISE, "isr raise", |cpu, ctx, _| {
        log(cpu, ctx, ISR_RAISE)?;
        // Raised while in service: must wait until this one returns.
        let raised = ctx.runtime().raise_interrupt(irq(ISR, &[2]));
        assert_eq!(raised, Raise::Queued);
        assert!(ctx.in_interrupt());
        // A safe point inside the callback: the new one must not enter here.
        Ok(TransferAbi::jump(ISR_TAIL))
    });
    let _ = d.register_override(ISR_TAIL, "isr tail", |cpu, ctx, _| {
        log(cpu, ctx, ISR_TAIL)?;
        ctx.memory().ret(cpu, 0)
    });
    let rt = Runtime::new(g, d);
    let mut cpu = CpuAbi::xbox_thread();
    cpu.esp = STACK_TOP - 4;
    (cpu.eax, cpu.ebx, cpu.esi) = (0x1111, 0x2222, 0x3333);
    cpu.eflags |= DF;
    cpu.x87.st[0] = [0x11; 10];
    cpu.mxcsr = 0x1fa0;
    (rt, cpu)
}
fn irq(target: u32, args: &[u32]) -> GuestInterrupt {
    GuestInterrupt {
        target,
        args: args.to_vec(),
        convention: CallConvention::Cdecl,
        thread: None,
        coalesce: false,
        name: "test",
    }
}
fn run(cpu: &mut CpuAbi, ctx: &mut ExecCtx) -> TransferAbi {
    Runner::default().run(cpu, ctx, MAIN)
}
fn exited(ctx: &ExecCtx, t: TransferAbi) -> bool {
    t.kind == abi::STOP && ctx.event(t.event_id) == Some(&Event::HarnessExit)
}

#[test]
fn queued_interrupt_runs_at_the_first_safe_point_with_state_restored() {
    let (rt, mut cpu) = setup(3);
    assert_eq!(rt.raise_interrupt(irq(ISR, &[0xabc, 0xdef])), Raise::Queued);
    assert_eq!(rt.pending_interrupts(), 1);
    let start = cpu;
    let mut ctx = ExecCtx::new(rt.clone(), 1);
    let t = run(&mut cpu, &mut ctx);
    assert!(exited(&ctx, t), "{:?}", ctx.events());
    assert_eq!(rt.pending_interrupts(), 0);
    // Delivered before MAIN ran at all, entered with DF clear and its argument.
    assert_eq!(entries(&rt), vec![[ISR, 0xabc, 0, 0]]);
    // Everything but EIP and the popped return slot is as MAIN left it.
    let mut expect = start;
    expect.esp = STACK_TOP;
    expect.eip = HARNESS_EXIT;
    assert_eq!(cpu, expect);
    assert!(!ctx.in_interrupt());
    assert_eq!(ctx.events().len(), 1);
}

#[test]
fn stdcall_callbacks_pop_their_arguments() {
    let (rt, mut cpu) = setup(2);
    rt.raise_interrupt(GuestInterrupt {
        convention: CallConvention::Stdcall,
        ..irq(ISR_STD, &[5, 6])
    });
    let mut ctx = ExecCtx::new(rt.clone(), 1);
    let t = run(&mut cpu, &mut ctx);
    assert!(exited(&ctx, t), "{:?}", ctx.events());
    assert_eq!(entries(&rt), vec![[ISR_STD, 5, 0, 0]]);
    assert_eq!((cpu.esp, cpu.eax, cpu.ebx), (STACK_TOP, 0x1111, 0x2222));
}

#[test]
fn an_unbalanced_return_faults_with_state_restored() {
    let (rt, mut cpu) = setup(2);
    rt.raise_interrupt(irq(ISR_BAD, &[1]));
    let start = cpu;
    let mut ctx = ExecCtx::new(rt.clone(), 1);
    let t = run(&mut cpu, &mut ctx);
    assert_eq!(t.kind, abi::FAULT);
    let expected_esp = STACK_TOP - 8;
    assert_eq!(
        ctx.event(t.event_id),
        Some(&Event::InterruptImbalance {
            name: "test",
            target: ISR_BAD,
            expected_esp,
            actual_esp: expected_esp + 4,
        })
    );
    // The snapshot is the state at the safe point, before MAIN was entered.
    assert_eq!(cpu, start);
}

#[test]
fn delivery_does_not_nest() {
    let (rt, mut cpu) = setup(4);
    rt.raise_interrupt(irq(ISR_RAISE, &[1]));
    let mut ctx = ExecCtx::new(rt.clone(), 1);
    let t = run(&mut cpu, &mut ctx);
    assert!(exited(&ctx, t), "{:?}", ctx.events());
    // The JUMP inside the first callback was a safe point, but the second
    // callback ran only after the first returned, still before MAIN's first
    // step.
    assert_eq!(
        entries(&rt),
        vec![[ISR_RAISE, 1, 0, 0], [ISR_TAIL, 1, 0, 0], [ISR, 2, 0, 0]]
    );
}

#[test]
fn masking_and_thread_filters_defer_without_dropping() {
    let (rt, mut cpu) = setup(2);
    rt.raise_interrupt(GuestInterrupt {
        thread: Some(7),
        ..irq(ISR, &[7])
    });
    rt.raise_interrupt(irq(ISR, &[1]));
    let mut ctx = ExecCtx::new(rt.clone(), 1);
    ctx.set_interrupts_masked(true);
    let start = cpu;
    let t = run(&mut cpu, &mut ctx);
    assert!(exited(&ctx, t));
    assert!(entries(&rt).is_empty());
    assert_eq!(rt.pending_interrupts(), 2);

    // Unmasked on thread 1, only the untargeted one is taken.
    ctx.set_interrupts_masked(false);
    rt.memory().set_u32(COUNT, 0).unwrap();
    rt.memory().set_u32(STACK_TOP - 4, HARNESS_EXIT).unwrap();
    cpu = start;
    let t = run(&mut cpu, &mut ctx);
    assert!(exited(&ctx, t));
    assert_eq!(entries(&rt), vec![[ISR, 1, 0, 0]]);
    assert_eq!(rt.pending_interrupts(), 1);

    let mut thread7 = ExecCtx::new(rt.clone(), 7);
    rt.memory().set_u32(COUNT, 0).unwrap();
    rt.memory().set_u32(STACK_TOP - 4, HARNESS_EXIT).unwrap();
    cpu = start;
    let t = run(&mut cpu, &mut thread7);
    assert!(exited(&thread7, t));
    assert_eq!(entries(&rt), vec![[ISR, 1, 0, 0], [ISR, 7, 0, 0]]);
    assert_eq!(rt.pending_interrupts(), 0);
}

#[test]
fn coalescing_and_the_queue_bound() {
    let (rt, _) = setup(0);
    let vblank = GuestInterrupt {
        coalesce: true,
        ..irq(ISR, &[])
    };
    assert_eq!(rt.raise_interrupt(vblank.clone()), Raise::Queued);
    assert_eq!(rt.raise_interrupt(vblank.clone()), Raise::Coalesced);
    // A different thread is a different interrupt.
    let other = GuestInterrupt {
        thread: Some(2),
        ..vblank.clone()
    };
    assert_eq!(rt.raise_interrupt(other), Raise::Queued);
    for _ in 2..MAX_PENDING {
        assert_eq!(rt.raise_interrupt(irq(ISR, &[])), Raise::Queued);
    }
    assert_eq!(rt.raise_interrupt(irq(ISR, &[])), Raise::Full);
    assert_eq!(rt.clear_interrupts(), MAX_PENDING);
    assert_eq!(rt.pending_interrupts(), 0);
    assert_eq!(rt.raise_interrupt(vblank), Raise::Queued);
}

#[test]
fn a_frame_that_cannot_be_pushed_faults_and_restores_state() {
    let (rt, mut cpu) = setup(1);
    rt.raise_interrupt(irq(ISR, &[1, 2]));
    // Only the return slot is mapped below ESP... none of the argument slots.
    cpu.esp = STACK;
    rt.memory().set_u32(STACK, HARNESS_EXIT).unwrap();
    let start = cpu;
    let mut ctx = ExecCtx::new(rt.clone(), 1);
    let t = run(&mut cpu, &mut ctx);
    assert_eq!(t.kind, abi::FAULT);
    assert!(matches!(
        ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
    // The snapshot is the state at the safe point, before MAIN was entered.
    assert_eq!(cpu, start);
    assert!(entries(&rt).is_empty());
}

#[test]
fn an_interrupt_raised_from_another_thread_is_delivered_mid_run() {
    // MAIN spins until the callback has run, as a vblank wait would.
    let (rt, mut cpu) = setup(u32::MAX);
    rt.edit_dispatch(|d| {
        let _ = d.register_override(MAIN, "wait", |cpu, ctx, _| {
            let mut m = ctx.memory();
            if m.u32(LOG)? != 0 {
                return m.ret(cpu, 0);
            }
            let n = m.u32(COUNT)?;
            m.set_u32(COUNT, n + 1)?;
            Ok(TransferAbi::jump(MAIN))
        });
    });
    let raiser = {
        let rt = rt.clone();
        std::thread::spawn(move || {
            while rt.memory().u32(COUNT).unwrap() < 1000 {
                std::thread::yield_now();
            }
            rt.raise_interrupt(irq(ISR, &[9]))
        })
    };
    let mut ctx = ExecCtx::new(rt.clone(), 1);
    let runner = Runner {
        max_transfers: u64::MAX,
    };
    let t = runner.run(&mut cpu, &mut ctx, MAIN);
    assert_eq!(raiser.join().unwrap(), Raise::Queued);
    assert!(exited(&ctx, t), "{:?}", ctx.events());
    let log = entries(&rt);
    assert_eq!(log.len(), 1);
    assert_eq!(log[0][..3], [ISR, 9, 0]);
    assert!(log[0][3] >= 1000);
    assert_eq!((cpu.eax, cpu.ebx, cpu.esi), (0x1111, 0x2222, 0x3333));
}
