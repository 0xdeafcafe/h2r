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
    /// The module's own probe or its table header disagrees with the runtime.
    #[error("generated module ABI: probe {probe}, table {table}, runtime {runtime}")]
    ModuleAbiVersion {
        probe: u32,
        table: u32,
        runtime: u32,
    },
    /// The module asks for features this runtime does not implement.
    #[error("generated module features {0:#x} are unknown to this runtime")]
    UnknownFeatures(u32),
    /// A table pointer is null while its count is not zero.
    #[error("generated module {0} table is null with a non-zero count")]
    NullTable(ModuleTable),
    /// The row after the last counted one is not the zero terminator, so the
    /// count and the table disagree.
    #[error("generated module {0} table has no zero row after its count")]
    MissingTerminator(ModuleTable),
    /// One row is unusable; nothing from the module was registered.
    #[error("generated module {table} row {index}: {error}")]
    ModuleRow {
        table: ModuleTable,
        index: usize,
        error: Box<RegistrationError>,
    },
    #[error("entry {addr:#010x} has unknown kind {kind}")]
    EntryKind { addr: u32, kind: u32 },
    #[error("entry {0:#010x} has a null handler or version probe")]
    NullEntry(u32),
    #[error("address zero cannot be registered")]
    ZeroAddress,
    #[error("resume owner {0:#010x} is not a function entry (kind 0) of this module")]
    OwnerNotFunction(u32),
}

/// Which table of an `H2GenModule` a registration error refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleTable {
    Entries,
    Resumes,
}
impl std::fmt::Display for ModuleTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Entries => "entry",
            Self::Resumes => "resume",
        })
    }
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

    /// Time an existing override at its actual boundary, preserving enable and
    /// replacement semantics. Call once after registration (before running).
    /// Use D3d for guest API entries; Gfx for a backend submission adapter;
    /// Fallback for an interpreter entry. Generated resumes are unaffected.
    pub fn profile_override(&mut self, addr: u32, scope: crate::perf::Scope) -> bool {
        let Some(over) = self.overrides.get_mut(&addr) else {
            return false;
        };
        let handler = over.handler.clone();
        over.handler = Handler::Rust(Arc::new(move |cpu, ctx, selector| {
            Ok(ctx.measure(scope, |ctx| {
                crate::handler::execute(
                    crate::handler::Invocation::Handler(handler.clone()),
                    cpu,
                    ctx,
                    selector,
                )
            }))
        }));
        true
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
    pub fn has_generated(&self, addr: u32) -> bool {
        self.generated.contains_key(&addr)
    }
    pub fn has_resume(&self, pc: u32) -> bool {
        self.resumes.contains_key(&pc)
    }
    pub(crate) fn resolve_original(&self, addr: u32) -> Option<Handler> {
        self.generated.get(&addr).cloned()
    }
    /// The owner's original handler, its selector and the owner address.
    pub(crate) fn resolve_resume(&self, pc: u32) -> Option<(Handler, u32, u32)> {
        let &(owner, selector) = self.resumes.get(&pc)?;
        Some((self.resolve_original(owner)?, selector, owner))
    }
}
