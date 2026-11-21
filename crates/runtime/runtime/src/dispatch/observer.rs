//! Temporary observation of a call and its result on the owning executor.
//! Installing and removing through Runtime::edit_dispatch also updates the
//! generated direct-call table, so the unobserved entry keeps its fast path.

use super::{Dispatch, Handler, Override};
use crate::{CpuAbi, Guest, TransferAbi, abi, exec::OriginalHandler};
use std::sync::Arc;

pub struct ReturnObservation {
    addr: u32,
    previous: Option<Override>,
    observer: Handler,
}

impl ReturnObservation {
    /// Restore only the entry this observation installed.
    pub fn remove(self, dispatch: &mut Dispatch) {
        let Some(current) = dispatch.overrides.get(&self.addr) else {
            return;
        };
        if let (Handler::Rust(current), Handler::Rust(observer)) =
            (&current.handler, &self.observer)
            && Arc::ptr_eq(current, observer)
        {
            if let Some(previous) = self.previous {
                dispatch.restore_override(self.addr, previous);
            } else {
                dispatch.overrides.remove(&self.addr);
            }
        }
    }
}

impl Dispatch {
    /// Observe a named entry until the returned handle is removed. The
    /// before callback copies data that belongs to this invocation. Its
    /// after callback sees that call's return, including for nested calls.
    /// An unreadable stack or a declined observation runs the body directly.
    pub fn observe_returns<T: Send + 'static>(
        &mut self,
        addr: u32,
        name: &'static str,
        before: impl Fn(&CpuAbi, &Guest) -> Option<T> + Send + Sync + 'static,
        after: impl Fn(T, &CpuAbi, &Guest) + Send + Sync + 'static,
    ) -> Option<ReturnObservation> {
        let handler = self.resolve(addr)?;
        let native = matches!(handler, Handler::Native(_) | Handler::NativeAdapter(_));
        let body = OriginalHandler(handler);
        let after = Arc::new(after);
        let observer = Handler::Rust(Arc::new(move |cpu, ctx, selector| {
            let rt = Arc::clone(ctx.runtime());
            {
                let mut guest = rt.memory();
                let sp = cpu.esp;
                if let Ok(caller) = guest.u32(sp)
                    && let Some(sample) = before(cpu, &guest)
                {
                    let after = after.clone();
                    // Replace the return slot with a continuation. Invoke the
                    // resolved body below, with that slot at the original ESP.
                    cpu.esp = sp.wrapping_add(4);
                    let call =
                        ctx.call_guest_named(cpu, &mut guest, addr, name, move |cpu, ctx| {
                            let mut guest = ctx.memory();
                            // Retail RET leaves the caller's address in the consumed
                            // slot. Remove our token if this is still its slot.
                            if guest.u32(sp) == Ok(cpu.eip) {
                                let _ = guest.set_u32(sp, caller);
                            }
                            after(sample, cpu, &guest);
                            Ok(TransferAbi::ret(caller))
                        });
                    if !call.is_ok_and(|transfer| transfer.kind == abi::CALL) {
                        cpu.esp = sp;
                    }
                }
            }
            let next = body.invoke(cpu, ctx, selector);
            // The runner sees our Rust wrapper. Forward the native body's
            // STOP through its usual generated-code recovery hook.
            if native
                && next.kind == abi::STOP
                && next.valid_abi()
                && let Some(recover) = rt.stop_recovery()
            {
                return Ok(recover(cpu, ctx, next));
            }
            Ok(next)
        }));
        let previous = self.register_override_handler(addr, name.to_owned(), observer.clone());
        // An enabled native body may resume internally at original PCs. Its
        // implementation owns those returns throughout the observation.
        if let Some(previous) = previous.as_ref().filter(|p| p.enabled) {
            self.overrides.get_mut(&addr)?.resume_body = previous.resume_body.clone();
        }
        Some(ReturnObservation {
            addr,
            previous,
            observer,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removing_a_return_observer_releases_a_generated_entry_for_direct_calls() {
        let mut dispatch = Dispatch::default();
        let addr = 0x1000;
        let observe = |dispatch: &mut Dispatch| {
            dispatch.observe_returns(addr, "observer", |_, _| Some(()), |(), _, _| {})
        };
        assert!(observe(&mut dispatch).is_none());
        dispatch
            .register_generated_rust(addr, |_, _, _| Ok(TransferAbi::stop(0)))
            .unwrap();
        let handle = observe(&mut dispatch).unwrap();
        assert_eq!(dispatch.override_name(addr), Some("observer"));
        handle.remove(&mut dispatch);
        assert_eq!(dispatch.override_name(addr), None);
        let handle = observe(&mut dispatch).unwrap();
        dispatch.register_override(addr, "other feature", |_, _, _| Ok(TransferAbi::stop(0)));
        handle.remove(&mut dispatch);
        assert_eq!(dispatch.override_name(addr), Some("other feature"));
    }
}
