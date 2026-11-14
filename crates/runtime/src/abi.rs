//! Fixed-width control transport. Tags remain integers so malformed C output
//! can be diagnosed without constructing an invalid Rust enum.

use crate::{CpuAbi, ExecCtxOpaque, cpu::ABI_VERSION};

pub const CALL: u32 = 1;
pub const JUMP: u32 = 2;
pub const RETURN: u32 = 3;
pub const FAULT: u32 = 4;
pub const STOP: u32 = 5;
pub const ENTRY: u32 = 0;
pub const RESUME: u32 = 1;
/// A guest RET first checks the exact resume map, then recognized entries.
pub const RETURN_TARGET: u32 = 2;

/// ABI v1: 28 bytes, alignment 4. `resume_pc` describes a CALL's pushed
/// continuation; it never substitutes for reading the actual guest stack.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferAbi {
    pub abi_version: u32,
    pub struct_size: u32,
    pub kind: u32,
    pub route: u32,
    pub target_pc: u32,
    pub resume_pc: u32,
    /// Opaque retained diagnostic id; resolve through ExecCtx::event. Zero means none.
    pub event_id: u32,
}

impl TransferAbi {
    pub const fn new(kind: u32, route: u32, target_pc: u32, resume_pc: u32, event_id: u32) -> Self {
        Self {
            abi_version: ABI_VERSION,
            struct_size: size_of::<Self>() as u32,
            kind,
            route,
            target_pc,
            resume_pc,
            event_id,
        }
    }
    pub const fn call(target: u32, resume: u32) -> Self {
        Self::new(CALL, ENTRY, target, resume, 0)
    }
    pub const fn jump(target: u32) -> Self {
        Self::new(JUMP, ENTRY, target, 0, 0)
    }
    pub const fn resume(target: u32) -> Self {
        Self::new(JUMP, RESUME, target, 0, 0)
    }
    pub const fn ret(target: u32) -> Self {
        Self::new(RETURN, RETURN_TARGET, target, 0, 0)
    }
    pub const fn stop(event: u32) -> Self {
        Self::new(STOP, ENTRY, 0, 0, event)
    }
    pub const fn fault(event: u32) -> Self {
        Self::new(FAULT, ENTRY, 0, 0, event)
    }
    pub fn valid_abi(&self) -> bool {
        self.abi_version == ABI_VERSION && self.struct_size == size_of::<Self>() as u32
    }
}

/// Registration is unsafe because the runner must trust C to respect pointer
/// lifetimes, fully spill canonical state, and never unwind across this call.
pub type GuestFn = unsafe extern "C" fn(*mut CpuAbi, *mut ExecCtxOpaque, u32) -> TransferAbi;
