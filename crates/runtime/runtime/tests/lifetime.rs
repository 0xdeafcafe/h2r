mod common;
use common::*;
use runtime::{CpuAbi, Event, ExecCtx, Runner, TransferAbi, abi, exec::HARNESS_EXIT, load_xbe};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn returning_guest(d: &mut runtime::Dispatch) {
    let _ = d.register_override(WRAPPER, "returning callback", |cpu, ctx, _| {
        ctx.memory().ret(cpu, 0)
    });
}
fn callback(ctx: &mut ExecCtx, cpu: &mut CpuAbi) -> TransferAbi {
    let runtime = Arc::clone(ctx.runtime());
    ctx.call_guest(cpu, &mut runtime.memory(), WRAPPER, |cpu, _| {
        cpu.eax += 1;
        Ok(TransferAbi::stop(0))
    })
    .unwrap()
}

#[test]
fn long_lived_context_keeps_stopping_and_exiting_after_retention_wraps() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    cpu.esp -= 64;
    ctx.memory().set_u32(cpu.esp, HARNESS_EXIT).unwrap();
    let runner = Runner::default();
    let token = runtime::kernel::function_token(255);
    let mut first = 0;
    for i in 0..runtime::events::MAX_EVENTS * 4 {
        let stop = runner.run(&mut cpu, &mut ctx, token);
        if i == 0 {
            first = stop.event_id;
        }
        assert_eq!(stop.kind, abi::STOP);
        assert!(
            matches!(ctx.event(stop.event_id), Some(Event::KernelCall(call)) if call.ordinal == 255)
        );
    }
    assert_eq!(ctx.events().len(), runtime::events::MAX_EVENTS);
    assert!(ctx.event(first).is_none());
    let exit = runner.run_from(&mut cpu, &mut ctx, TransferAbi::ret(HARNESS_EXIT));
    assert_eq!(exit.kind, abi::STOP);
    assert_eq!(ctx.event(exit.event_id), Some(&Event::HarnessExit));
    let taken = ctx.take_events();
    assert_eq!(taken.len(), runtime::events::MAX_EVENTS);
    assert!(matches!(taken.last(), Some((id, Event::HarnessExit)) if *id == exit.event_id));
    assert!(ctx.events().is_empty());
    let next = ctx.emit(Event::TransferLimit);
    assert_ne!(next, first);
    assert!(ctx.event(exit.event_id).is_none());
    assert_eq!(ctx.event(next), Some(&Event::TransferLimit));
}

#[test]
fn completed_callbacks_recycle_tokens_beyond_the_old_lifetime_limit() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    image.runtime.edit_dispatch(returning_guest);
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    let runner = Runner::default();
    // The old monotonic 1 MB token range exhausted at exactly 262,144 calls.
    for _ in 0..262_145 {
        let call = callback(&mut ctx, &mut cpu);
        assert_eq!(runner.run_from(&mut cpu, &mut ctx, call).kind, abi::STOP);
    }
    assert_eq!(cpu.eax, 262_145);
    assert_eq!(cpu.esp, image.cpu.esp);
    assert!(ctx.events().is_empty());
}

#[test]
fn a_rust_continuation_can_schedule_another_callback() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    image.runtime.edit_dispatch(returning_guest);
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    let runtime = Arc::clone(ctx.runtime());
    let first = ctx
        .call_guest(&mut cpu, &mut runtime.memory(), WRAPPER, |cpu, ctx| {
            cpu.eax = 10;
            let runtime = Arc::clone(ctx.runtime());
            ctx.call_guest(cpu, &mut runtime.memory(), WRAPPER, |cpu, _| {
                cpu.eax += 20;
                Ok(TransferAbi::stop(0))
            })
        })
        .unwrap();
    assert_eq!(
        Runner::default().run_from(&mut cpu, &mut ctx, first).kind,
        abi::STOP
    );
    assert_eq!(cpu.eax, 30);
    assert_eq!(cpu.esp, image.cpu.esp);
    assert_eq!(ctx.clear_continuations(), 0);
}

#[test]
fn interleaved_overrides_keep_two_distinct_live_return_tokens() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    image.runtime.edit_dispatch(|d| {
        let _ = d.register_override(ENTRY, "outer override", |cpu, ctx, _| {
            let runtime = Arc::clone(ctx.runtime());
            ctx.call_guest(cpu, &mut runtime.memory(), WRAPPER, |cpu, _| {
                cpu.eax = cpu.eax * 10 + 1;
                Ok(TransferAbi::stop(0))
            })
        });
        let _ = d.register_override(WRAPPER, "inner override", |cpu, ctx, _| {
            cpu.ebx = ctx.memory().u32(cpu.esp)?;
            let runtime = Arc::clone(ctx.runtime());
            ctx.call_guest(cpu, &mut runtime.memory(), WRAPPER + 0x10, |cpu, ctx| {
                cpu.eax = cpu.eax * 10 + 2;
                ctx.memory().ret(cpu, 0)
            })
        });
        let _ = d.register_override(WRAPPER + 0x10, "leaf guest", |cpu, ctx, _| {
            cpu.edx = ctx.memory().u32(cpu.esp)?;
            ctx.memory().ret(cpu, 0)
        });
    });
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    assert_eq!(
        Runner::default().run(&mut cpu, &mut ctx, ENTRY).kind,
        abi::STOP
    );
    assert_ne!(cpu.ebx, cpu.edx);
    assert_eq!(cpu.eax, 21);
    assert_eq!(cpu.esp, image.cpu.esp);
    assert_eq!(ctx.clear_continuations(), 0);
}

struct Dropped(Arc<AtomicUsize>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn cancellation_clear_and_jump_release_saved_state_and_recycle_tokens() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    let runtime = Arc::clone(ctx.runtime());
    let drops = Arc::new(AtomicUsize::new(0));
    let mut tokens = Vec::new();
    for _ in 0..3 {
        let owned = Dropped(Arc::clone(&drops));
        let call = ctx
            .call_guest(&mut cpu, &mut runtime.memory(), WRAPPER, move |_, _| {
                drop(owned);
                Ok(TransferAbi::stop(0))
            })
            .unwrap();
        tokens.push(call.resume_pc);
    }
    assert!(ctx.cancel_continuation(tokens[1]));
    assert!(!ctx.cancel_continuation(tokens[1]));
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    let fault = Runner::default().run_from(&mut cpu, &mut ctx, TransferAbi::jump(tokens[0]));
    assert!(
        matches!(ctx.event(fault.event_id), Some(Event::UnsupportedTarget { target, kind: abi::JUMP, .. }) if *target == tokens[0])
    );
    assert_eq!(drops.load(Ordering::SeqCst), 2);
    assert_eq!(ctx.clear_continuations(), 1);
    assert_eq!(ctx.clear_continuations(), 0);
    assert_eq!(drops.load(Ordering::SeqCst), 3);
    // Abandoned guest frames are discarded before reusing the context.
    cpu.esp = image.cpu.esp;
    let next = callback(&mut ctx, &mut cpu);
    assert!(!tokens.contains(&next.resume_pc));
    assert!(ctx.cancel_continuation(next.resume_pc));
}

#[test]
fn abandoned_callbacks_can_be_cleared_repeatedly_without_a_lifetime_limit() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    for _ in 0..3 {
        for _ in 0..1024 {
            assert_eq!(callback(&mut ctx, &mut cpu).kind, abi::CALL);
        }
        assert_eq!(callback(&mut ctx, &mut cpu).kind, abi::FAULT);
        assert_eq!(ctx.clear_continuations(), 1024);
        cpu.esp = image.cpu.esp;
    }
}

#[test]
fn failed_stack_push_does_not_lose_its_reserved_token() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    cpu.esp = 0;
    assert!(callback_fallible(&mut ctx, &mut cpu).is_err());
    assert_eq!(ctx.clear_continuations(), 0);
    cpu.esp = image.cpu.esp;
    let call = callback(&mut ctx, &mut cpu);
    assert!(ctx.cancel_continuation(call.resume_pc));
}
fn callback_fallible(
    ctx: &mut ExecCtx,
    cpu: &mut CpuAbi,
) -> Result<TransferAbi, runtime::GuestFault> {
    let runtime = Arc::clone(ctx.runtime());
    ctx.call_guest(cpu, &mut runtime.memory(), WRAPPER, |_, _| {
        Ok(TransferAbi::stop(0))
    })
}

#[derive(Clone, Copy)]
enum RetireOld {
    Return,
    Cancel,
    Clear,
}

fn stale_token_cannot_affect_replacement(retirement: RetireOld, jump_stale: bool) {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    let runtime = Arc::clone(ctx.runtime());
    let old_runs = Arc::new(AtomicUsize::new(0));
    let captured = Arc::clone(&old_runs);
    let old = ctx
        .call_guest(&mut cpu, &mut runtime.memory(), WRAPPER, move |_, _| {
            captured.fetch_add(1, Ordering::SeqCst);
            Ok(TransferAbi::stop(0))
        })
        .unwrap()
        .resume_pc;
    // Occupy every other slot. After retiring `old`, the new call must use
    // exactly that slot after RET/cancel: no other slots are available. Clear
    // releases the whole pool in FIFO order, starting with that same slot.
    for _ in 1..1024 {
        assert_eq!(callback(&mut ctx, &mut cpu).kind, abi::CALL);
    }
    let runner = Runner::default();
    match retirement {
        RetireOld::Cancel => assert!(ctx.cancel_continuation(old)),
        RetireOld::Return => assert_eq!(
            runner
                .run_from(&mut cpu, &mut ctx, TransferAbi::ret(old))
                .kind,
            abi::STOP
        ),
        RetireOld::Clear => assert_eq!(ctx.clear_continuations(), 1024),
    }
    assert_eq!(
        old_runs.load(Ordering::SeqCst),
        usize::from(matches!(retirement, RetireOld::Return))
    );
    cpu.esp = image.cpu.esp;
    let new_runs = Arc::new(AtomicUsize::new(0));
    let captured = Arc::clone(&new_runs);
    let new = ctx
        .call_guest(&mut cpu, &mut runtime.memory(), WRAPPER, move |_, _| {
            captured.fetch_add(1, Ordering::SeqCst);
            Ok(TransferAbi::stop(0))
        })
        .unwrap()
        .resume_pc;
    let decoy_runs = Arc::new(AtomicUsize::new(0));
    let captured = Arc::clone(&decoy_runs);
    // A stale control token cannot fall through to entry dispatch, even if
    // somebody registers an override and an executable mapping at that PC.
    runtime
        .memory()
        .map_zeroed(old & !4095, 4096, runtime::guest::Permissions::RX)
        .unwrap();
    runtime.edit_dispatch(|d| {
        let _ = d.register_override(old, "stale token decoy", move |_, _, _| {
            captured.fetch_add(1, Ordering::SeqCst);
            Ok(TransferAbi::stop(0))
        });
    });
    let stale = if jump_stale {
        TransferAbi::jump(old)
    } else {
        TransferAbi::ret(old)
    };
    let outcome = runner.run_from(&mut cpu, &mut ctx, stale);
    assert_eq!(outcome.kind, abi::FAULT);
    assert!(
        matches!(ctx.event(outcome.event_id), Some(Event::UnsupportedTarget { target, kind, .. })
        if *target == old && *kind == stale.kind)
    );
    assert_eq!(new_runs.load(Ordering::SeqCst), 0);
    assert_eq!(decoy_runs.load(Ordering::SeqCst), 0);
    assert!(!ctx.cancel_continuation(old));
    // Neither stale RET, stale JUMP nor stale cancellation consumed the new
    // closure. It remains resumable using its own actual guest stack token.
    let valid_return = ctx.memory().ret(&mut cpu, 0).unwrap();
    assert_eq!(valid_return.target_pc, new);
    assert_eq!(
        runner.run_from(&mut cpu, &mut ctx, valid_return).kind,
        abi::STOP
    );
    assert_eq!(new_runs.load(Ordering::SeqCst), 1);
    assert_ne!(old, new);
    assert_eq!(
        ctx.clear_continuations(),
        if matches!(retirement, RetireOld::Clear) {
            0
        } else {
            1023
        }
    );
}

#[test]
fn ret_to_consumed_token_cannot_resume_a_new_closure_in_the_same_slot() {
    stale_token_cannot_affect_replacement(RetireOld::Return, false);
}
#[test]
fn jump_to_consumed_token_cannot_cancel_a_new_closure_in_the_same_slot() {
    stale_token_cannot_affect_replacement(RetireOld::Return, true);
}
#[test]
fn ret_to_cancelled_token_cannot_resume_a_new_closure_in_the_same_slot() {
    stale_token_cannot_affect_replacement(RetireOld::Cancel, false);
}
#[test]
fn jump_to_cancelled_token_cannot_cancel_a_new_closure_in_the_same_slot() {
    stale_token_cannot_affect_replacement(RetireOld::Cancel, true);
}

#[test]
fn harness_exit_has_normal_history_retention_after_it_is_reported() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    let stopped = Runner::default().run_from(&mut cpu, &mut ctx, TransferAbi::ret(HARNESS_EXIT));
    assert_eq!(ctx.event(stopped.event_id), Some(&Event::HarnessExit));
    for _ in 0..runtime::events::MAX_EVENTS {
        ctx.emit(Event::TransferLimit);
    }
    assert!(ctx.event(stopped.event_id).is_none());
}

#[test]
fn ret_to_cleared_token_cannot_resume_a_new_closure_in_the_same_slot() {
    stale_token_cannot_affect_replacement(RetireOld::Clear, false);
}
#[test]
fn jump_to_cleared_token_cannot_cancel_a_new_closure_in_the_same_slot() {
    stale_token_cannot_affect_replacement(RetireOld::Clear, true);
}
