//! The runner's preemption safe point (src/preempt.rs): the word generated
//! loops poll, the access-counted slice, and what armed and unarmed
//! contexts do when the word is set.
use runtime::{
    CpuAbi, Dispatch, ExecCtx, ExecCtxOpaque, Runner, Runtime, TransferAbi, abi, ffi,
    guest::{Guest, Permissions},
    preempt,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

const LOOP: u32 = 0x40_1000;
const DATA: u32 = 0x50_0000;

/// A loop made of transfers: LOOP jumps to itself until [DATA] is set,
/// then returns to the harness. Each trip passes the runner's safe point.
fn setup() -> (Arc<Runtime>, CpuAbi) {
    let mut guest = Guest::new();
    guest
        .map_zeroed(0x40_0000, 0x1_0000, Permissions::RX)
        .unwrap();
    guest
        .map_zeroed(0x10_0000, 0x1_0000, Permissions::RW)
        .unwrap();
    guest.map_zeroed(DATA, 0x1000, Permissions::RW).unwrap();
    guest
        .set_u32(0x11_0000 - 4, runtime::runner::HARNESS_EXIT)
        .unwrap();
    let mut dispatch = Dispatch::default();
    dispatch.register_override(LOOP, "loop", |cpu, ctx, _| {
        if ctx.memory().u32(DATA)? != 0 {
            return ctx.memory().ret(cpu, 0);
        }
        cpu.eax += 1;
        Ok(TransferAbi::jump(LOOP))
    });
    let cpu = CpuAbi {
        esp: 0x11_0000 - 4,
        ..CpuAbi::default()
    };
    (Runtime::new(guest, dispatch), cpu)
}

/// `*(const uint32_t *const *)ctx`, as `h2_preempt_word` reads it.
fn word_seen_by_c(ctx: &mut ExecCtx) -> u32 {
    let p = (ctx as *mut ExecCtx).cast::<*const AtomicU32>();
    // SAFETY: the context's first field is that pointer (ExecCtx is
    // repr(C) and asserts the offset), valid while the context lives.
    unsafe { (**p).load(Ordering::Relaxed) }
}

#[test]
fn the_first_field_of_a_context_points_at_its_runtimes_word() {
    let (rt, _) = setup();
    let mut ctx = ExecCtx::new(rt.clone(), 1);
    assert_eq!(word_seen_by_c(&mut ctx), 0);
    rt.request_preempt(preempt::REQUEST | preempt::INTERRUPT);
    assert_eq!(word_seen_by_c(&mut ctx), 6);
    assert_eq!(rt.preempt_pending(), 6);
    // Another runtime's word is its own.
    let (other, _) = setup();
    assert_eq!(word_seen_by_c(&mut ExecCtx::new(other, 1)), 0);
}

#[test]
fn an_armed_context_stops_at_the_safe_point_and_resumes_with_the_same_transfer() {
    let (rt, mut cpu) = setup();
    let mut ctx = ExecCtx::new(rt.clone(), 1);
    ctx.arm_preemption(0);
    rt.request_preempt(preempt::REQUEST);
    let runner = Runner { max_transfers: 100 };
    let t = runner.run(&mut cpu, &mut ctx, LOOP);
    assert_eq!((t.kind, t.event_id), (abi::STOP, 0));
    let resume = ctx.take_preempted().expect("a stashed transfer");
    assert_eq!(resume, TransferAbi::jump(LOOP), "nothing ran yet");
    assert_eq!(rt.preempt_pending(), 0, "the request was taken");
    assert_eq!(cpu.eax, 0);
    // Without a new request it simply carries on.
    rt.memory().set_u32(DATA, 1).unwrap();
    let t = runner.run_from(&mut cpu, &mut ctx, resume);
    assert_eq!(t.kind, abi::STOP);
    assert!(ctx.take_preempted().is_none());
    assert_eq!(cpu.eip, runtime::runner::HARNESS_EXIT);
}

#[test]
fn an_unarmed_context_drops_the_request_and_carries_on() {
    let (rt, mut cpu) = setup();
    let mut ctx = ExecCtx::new(rt.clone(), 1);
    rt.request_preempt(preempt::REQUEST | preempt::SLICE);
    let t = Runner { max_transfers: 5 }.run(&mut cpu, &mut ctx, LOOP);
    // It ran into the transfer limit instead of stopping for preemption.
    assert_eq!(t.kind, abi::FAULT);
    assert!(ctx.take_preempted().is_none());
    assert_eq!(rt.preempt_pending(), 0);
    assert_eq!(cpu.eax, 5);
}

#[test]
fn an_interrupt_alone_does_not_take_the_thread_away() {
    let (rt, mut cpu) = setup();
    let mut ctx = ExecCtx::new(rt.clone(), 1);
    ctx.arm_preemption(0);
    rt.request_preempt(preempt::INTERRUPT);
    rt.memory().set_u32(DATA, 1).unwrap();
    let t = Runner::default().run(&mut cpu, &mut ctx, LOOP);
    assert_eq!(t.kind, abi::STOP);
    assert!(ctx.take_preempted().is_none());
    assert_eq!(cpu.eip, runtime::runner::HARNESS_EXIT);
    assert_eq!(rt.preempt_pending(), 0);
}

#[test]
fn the_slice_is_spent_by_generated_memory_accesses() {
    let (rt, _) = setup();
    let mut ctx = ExecCtx::new(rt.clone(), 1);
    let read = |ctx: &mut ExecCtx| {
        let mut out = [0u8; 4];
        // SAFETY: an exclusive live context and a 4-byte output buffer.
        let st = unsafe {
            ffi::h2abi_v1_read(
                (ctx as *mut ExecCtx).cast::<ExecCtxOpaque>(),
                0,
                DATA,
                4,
                1,
                out.as_mut_ptr(),
            )
        };
        assert_eq!(st, 0);
    };
    // Unarmed: nothing is counted.
    for _ in 0..10 {
        read(&mut ctx);
    }
    assert_eq!(rt.preempt_pending(), 0);
    ctx.arm_preemption(3);
    read(&mut ctx);
    read(&mut ctx);
    assert_eq!(rt.preempt_pending(), 0);
    read(&mut ctx);
    assert_eq!(
        rt.preempt_pending(),
        preempt::SLICE,
        "the third access ends it"
    );
    ctx.disarm_preemption();
    assert!(!ctx.preemption_armed());
}

#[test]
fn raising_an_interrupt_asks_generated_loops_for_a_safe_point() {
    let (rt, _) = setup();
    assert_eq!(rt.preempt_pending(), 0);
    let raise = rt.raise_interrupt(runtime::GuestInterrupt {
        target: LOOP,
        args: vec![],
        convention: runtime::CallConvention::Cdecl,
        thread: None,
        coalesce: false,
        name: "vblank",
    });
    assert_eq!(raise, runtime::Raise::Queued);
    assert_eq!(rt.preempt_pending(), preempt::INTERRUPT);
}
