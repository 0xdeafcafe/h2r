//! Shared memory and dispatch services. Locks end before handler calls.
use crate::{Dispatch, Guest, TransferAbi, abi, dispatch::Handler, handler::OriginalHandler};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

pub struct Runtime {
    memory: Mutex<Guest>,
    dispatch: RwLock<Dispatch>,
}
impl Runtime {
    pub fn new(memory: Guest, dispatch: Dispatch) -> Arc<Self> {
        Arc::new(Self {
            memory: Mutex::new(memory),
            dispatch: RwLock::new(dispatch),
        })
    }
    /// Compound HLE operations use this guard, including Guest::call/ret.
    /// Release it before dispatching or invoking callbacks.
    pub fn memory(&self) -> MutexGuard<'_, Guest> {
        self.memory.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn edit_dispatch<R>(&self, f: impl FnOnce(&mut Dispatch) -> R) -> R {
        f(&mut self.dispatch.write().unwrap_or_else(|e| e.into_inner()))
    }
    pub(crate) fn resolve(&self, t: TransferAbi) -> Option<(Handler, u32)> {
        let d = self.dispatch.read().unwrap_or_else(|e| e.into_inner());
        match t.route {
            abi::ENTRY => d.resolve(t.target_pc).map(|h| (h, t.target_pc)),
            abi::RESUME => d.resolve_resume(t.target_pc),
            abi::RETURN_TARGET => d
                .resolve_resume(t.target_pc)
                .or_else(|| d.resolve(t.target_pc).map(|h| (h, t.target_pc))),
            _ => None,
        }
    }
    pub fn resolve_original(&self, pc: u32) -> Option<OriginalHandler> {
        self.dispatch
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .resolve_original(pc)
            .map(OriginalHandler)
    }
}
