//! Entry overrides and generated continuations are separate contracts. A
//! resolved handler owns its adapter, so Runtime drops its table lock before
//! invocation, including handlers which change dispatch or reenter services.

use crate::{CpuAbi, ExecCtx, GuestFault, GuestFn, TransferAbi};
use std::{collections::HashMap, sync::Arc};

type SafeFn =
    dyn Fn(&mut CpuAbi, &mut ExecCtx, u32) -> Result<TransferAbi, GuestFault> + Send + Sync;

#[derive(Clone)]
pub(crate) enum Handler {
    Native(GuestFn),
    Rust(Arc<SafeFn>),
}

#[derive(Clone)]
pub struct Override {
    pub name: String,
    handler: Handler,
    pub enabled: bool,
}

#[derive(Default)]
pub struct Dispatch {
    generated: HashMap<u32, Handler>,
    overrides: HashMap<u32, Override>,
    resumes: HashMap<u32, (u32, u32)>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RegistrationError {
    #[error("handler ABI version {0} does not match runtime ABI")]
    AbiVersion(u32),
    #[error("resume owner {0:#010x} has no generated original")]
    MissingOwner(u32),
    #[error("generated handler already registered at {0:#010x}")]
    DuplicateGenerated(u32),
    #[error("resume already registered at {0:#010x}")]
    DuplicateResume(u32),
}

impl Dispatch {
    pub(crate) fn register_generated_handler(
        &mut self,
        addr: u32,
        handler: Handler,
    ) -> Result<(), RegistrationError> {
        if self.generated.contains_key(&addr) {
            return Err(RegistrationError::DuplicateGenerated(addr));
        }
        self.generated.insert(addr, handler);
        Ok(())
    }
    pub(crate) fn register_override_handler(
        &mut self,
        addr: u32,
        name: String,
        handler: Handler,
    ) -> Option<Override> {
        self.overrides.insert(
            addr,
            Override {
                name,
                handler,
                enabled: true,
            },
        )
    }

    /// Restore a previously displaced override, returning the one it replaces.
    pub fn restore_override(&mut self, addr: u32, previous: Override) -> Option<Override> {
        self.overrides.insert(addr, previous)
    }

    /// A safe adapter owns captured state and has no FFI pointer obligations.
    pub fn register_override<F>(
        &mut self,
        addr: u32,
        name: impl Into<String>,
        f: F,
    ) -> Option<Override>
    where
        F: Fn(&mut CpuAbi, &mut ExecCtx, u32) -> Result<TransferAbi, GuestFault>
            + Send
            + Sync
            + 'static,
    {
        self.register_override_handler(addr, name.into(), Handler::Rust(Arc::new(f)))
    }

    /// Only exact validated PCs are accepted. `start_pc` selects the owner's
    /// generated label; it is not interpreted as an address interval.
    pub fn register_resume(
        &mut self,
        pc: u32,
        owner: u32,
        start_pc: u32,
    ) -> Result<(), RegistrationError> {
        if !self.generated.contains_key(&owner) {
            return Err(RegistrationError::MissingOwner(owner));
        }
        if self.resumes.contains_key(&pc) {
            return Err(RegistrationError::DuplicateResume(pc));
        }
        self.resumes.insert(pc, (owner, start_pc));
        Ok(())
    }
    pub fn set_enabled(&mut self, addr: u32, enabled: bool) -> bool {
        self.overrides
            .get_mut(&addr)
            .map(|o| o.enabled = enabled)
            .is_some()
    }
    pub fn override_name(&self, addr: u32) -> Option<&str> {
        self.overrides.get(&addr).map(|o| o.name.as_str())
    }
    pub(crate) fn resolve(&self, addr: u32) -> Option<Handler> {
        self.overrides
            .get(&addr)
            .filter(|o| o.enabled)
            .map(|o| o.handler.clone())
            .or_else(|| self.resolve_original(addr))
    }
    pub(crate) fn resolve_original(&self, addr: u32) -> Option<Handler> {
        self.generated.get(&addr).cloned()
    }
    pub(crate) fn resolve_resume(&self, pc: u32) -> Option<(Handler, u32)> {
        let &(owner, selector) = self.resumes.get(&pc)?;
        Some((self.resolve_original(owner)?, selector))
    }
}
