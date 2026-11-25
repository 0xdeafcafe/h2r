//! Passive return observers live on the guest stack's execution context.
//! The guest retains its real return word, including when two virtual
//! addresses name that word or the stack is read-only.
use crate::{CpuAbi, ExecCtx, TransferAbi, abi};

pub(crate) struct Observation {
    slot: u32,
    after: u32,
    target: u32,
    memory: (u64, u64),
    callback: Box<dyn FnOnce(&CpuAbi, &ExecCtx) + Send>,
}

impl ExecCtx {
    /// Observe the return from the current guest entry. `pop` is the
    /// argument cleanup in bytes, excluding the four-byte return address.
    /// Call before giving control to the original, with no memory guard
    /// held. The callback may inspect the canonical CPU and guest memory.
    ///
    /// An unreadable slot, an active access trap, a wrapping stack range
    /// or a full observer stack declines registration. A changed target,
    /// unexpected stack cleanup, unwind, fault or restored memory drops
    /// the observation. Preemption can suspend it with the guest thread.
    /// The callback is diagnostic, runs once at most, and neither saves a
    /// continuation nor modifies the return word.
    pub fn observe_guest_return(
        &mut self,
        cpu: &CpuAbi,
        pop: u32,
        observer: impl FnOnce(&CpuAbi, &ExecCtx) + Send + 'static,
    ) -> bool {
        let Some(after) = cpu.esp.checked_add(4).and_then(|sp| sp.checked_add(pop)) else {
            return false;
        };
        if self.return_observers.len() == crate::frames::MAX_RECORDS {
            return false;
        }
        let (target, memory) = {
            let g = self.runtime().memory();
            if g.has_trap() {
                return false;
            }
            let Ok(target) = g.u32(cpu.esp) else {
                return false;
            };
            (target, g.write_watch_epoch())
        };
        self.return_observers.push(Observation {
            slot: cpu.esp,
            after,
            target,
            memory,
            callback: Box::new(observer),
        });
        self.update_observed_depth();
        true
    }

    pub(crate) fn clear_return_observers(&mut self) {
        if !self.return_observers.is_empty() {
            self.return_observers.clear();
            self.update_observed_depth();
        }
    }

    #[inline]
    pub(crate) fn observe_transfer(&mut self, cpu: &CpuAbi, transfer: TransferAbi) {
        if !self.return_observers.is_empty() {
            self.unwind_observers(cpu, transfer);
        }
    }

    #[inline(never)]
    fn unwind_observers(&mut self, cpu: &CpuAbi, transfer: TransferAbi) {
        let mut returned_slot = None;
        while self.return_observers.last().is_some_and(|p| {
            // CALL reusing a slot has abandoned its previous owner. A tail
            // jump with ESP still at the slot keeps its caller's observer.
            p.slot < cpu.esp || (p.slot == cpu.esp && transfer.kind == abi::CALL)
        }) {
            let p = self.return_observers.pop().unwrap();
            // One RET popped one return word. Multiple entry hooks or tail
            // callees can observe that same word. Outer slots swept over
            // by an unusual RET cleanup are abandoned.
            if *returned_slot.get_or_insert(p.slot) == p.slot
                && transfer.kind == abi::RETURN
                && transfer.target_pc == p.target
                && cpu.esp == p.after
            {
                let valid = {
                    let g = self.runtime().memory();
                    !g.has_trap() && g.write_watch_epoch() == p.memory
                };
                if valid {
                    (p.callback)(cpu, self);
                }
            }
        }
        if self.return_observers.is_empty() {
            self.update_observed_depth();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Dispatch, Guest, Runner, Runtime, guest::Permissions, runner::HARNESS_EXIT};
    use std::sync::{Arc, Mutex};

    const CODE: u32 = 0x4000;
    const STACK: u32 = 0x21000;
    const ALIAS: u32 = 0x31000;

    fn context(dispatch: Dispatch) -> (ExecCtx, CpuAbi) {
        let mut g = Guest::new();
        g.map(CODE, 0x100, 0, Permissions::RX).unwrap();
        g.map(STACK - 0x100, 0x100, 0x1000, Permissions::RW)
            .unwrap();
        g.map(STACK, 0x100, 0x1100, Permissions::READ).unwrap();
        g.map(ALIAS, 0x100, 0x1100, Permissions::RW).unwrap();
        g.set_u32(ALIAS, HARNESS_EXIT).unwrap();
        let ctx = ExecCtx::new(Runtime::new(g, dispatch), 17);
        let cpu = CpuAbi {
            esp: STACK,
            ..CpuAbi::default()
        };
        (ctx, cpu)
    }

    #[test]
    fn a_read_only_return_slot_keeps_its_real_value_through_nested_calls() {
        const INNER: u32 = CODE + 0x10;
        const RESUME: u32 = CODE + 0x20;
        for diagnostics in [true, false] {
            let seen = Arc::new(Mutex::new(Vec::new()));
            let mut dispatch = Dispatch::default();
            let outer = Arc::clone(&seen);
            dispatch.register_override(CODE, "outer", move |c, ctx, _| {
                let outer = Arc::clone(&outer);
                assert!(ctx.observe_guest_return(c, 8, move |c, ctx| {
                    assert_eq!(ctx.thread_id, 17);
                    assert_eq!(ctx.runtime().memory().u32(ALIAS).unwrap(), HARNESS_EXIT);
                    outer.lock().unwrap().push((c.eax, c.esp));
                }));
                assert_eq!(ctx.memory().u32(ALIAS)?, HARNESS_EXIT);
                c.esp -= 4;
                ctx.memory().call(c, INNER, RESUME)
            });
            let inner = Arc::clone(&seen);
            dispatch.register_override(INNER, "inner", move |c, ctx, _| {
                let inner = Arc::clone(&inner);
                assert!(ctx.observe_guest_return(c, 4, move |c, _| {
                    inner.lock().unwrap().push((c.eax, c.esp));
                }));
                c.eax = 91;
                ctx.memory().ret(c, 4)
            });
            dispatch.register_override(RESUME, "resume", |c, ctx, _| {
                c.eax += 1;
                ctx.memory().ret(c, 8)
            });
            let (mut ctx, mut cpu) = context(dispatch);
            let done = Runner::default().run_from_instance(
                &mut cpu,
                &mut ctx,
                TransferAbi::jump(CODE),
                diagnostics,
            );
            assert_eq!(done.kind, abi::STOP);
            assert_eq!(*seen.lock().unwrap(), [(91, STACK), (92, STACK + 12)]);
            assert_eq!(ctx.memory().u32(ALIAS).unwrap(), HARNESS_EXIT);
            assert!(ctx.live_continuations().is_empty());
        }
    }

    #[test]
    fn a_changed_return_word_wrong_cleanup_or_stack_unwind_drops_the_observer() {
        let (mut ctx, mut c) = context(Dispatch::default());
        for case in 0..5 {
            c.esp = STACK;
            ctx.memory().set_u32(ALIAS, HARNESS_EXIT).unwrap();
            assert!(ctx.observe_guest_return(&c, 4, |_, _| panic!("abandoned call")));
            let t = match case {
                0 => {
                    ctx.memory().set_u32(ALIAS, CODE).unwrap();
                    ctx.memory().ret(&mut c, 4).unwrap()
                }
                1 => ctx.memory().ret(&mut c, 0).unwrap(),
                2 => {
                    c.esp += 8;
                    TransferAbi::jump(HARNESS_EXIT)
                }
                3 => TransferAbi::call(CODE, HARNESS_EXIT), // new call reuses the same slot
                _ => {
                    let layout = ctx.continuation_layout().unwrap();
                    ctx.restore_continuations(&layout, |_, _, _| None).unwrap();
                    ctx.memory().ret(&mut c, 4).unwrap()
                }
            };
            ctx.observe_transfer(&c, t);
            assert!(ctx.return_observers.is_empty());
        }
        c.esp = STACK;
        assert!(ctx.observe_guest_return(&c, 0, |_, _| panic!("restarted")));
        assert!(ctx.restart().return_observers.is_empty());
    }
}
