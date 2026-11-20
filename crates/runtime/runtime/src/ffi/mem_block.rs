//! The flat model's per-context block, h2fastmem.h's `H2Mem`
//! (docs/runtime/contract-thin-transfers-flat-ram.md, section 4.1, slice
//! F3). The context's third word points at it ([`crate::ExecCtx`]); a
//! flat-RAM handler loads `base` and `cards` once, at entry, and counts its
//! accesses in `accesses` against `budget` as a paged handler does in its
//! [`FastAccess`](super::FastAccess).
//!
//! The runtime fills `base`, `cards` and `budget` before it invokes a
//! handler ([`super::call_native`]) and takes `accesses` at every settle
//! ([`super::sync_accesses`]). A paged module never reads the block.

use std::ptr::NonNull;
use std::sync::atomic::AtomicU32;

/// h2fastmem.h `H2Mem`.
#[repr(C)]
pub struct H2Mem {
    /// Host address of guest address 0: the runtime's window, or the null
    /// window while the context's fast memory is off.
    pub base: *mut u8,
    /// One byte per 4 KiB guest page, one per 4 MiB region at
    /// `cards - 1024`; a store writes zero to both.
    pub cards: *mut u8,
    pub accesses: u64,
    pub budget: u64,
    pub preempt: *const AtomicU32,
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<H2Mem>() == 40);

/// Owns the block without reborrowing it through `Box` while generated C
/// holds its pointer (as `OwnedFastAccess` does): every access goes
/// through the raw pointer. One word, so it can be the context's third.
#[repr(transparent)]
pub(crate) struct MemBlock {
    ptr: NonNull<H2Mem>,
}

impl MemBlock {
    pub(crate) fn new() -> Self {
        let block = Box::new(H2Mem {
            base: std::ptr::null_mut(),
            cards: std::ptr::null_mut(),
            accesses: 0,
            budget: u64::MAX,
            preempt: std::ptr::null(),
        });
        Self {
            ptr: NonNull::new(Box::into_raw(block)).expect("Box is non-null"),
        }
    }
    fn as_ptr(&self) -> *mut H2Mem {
        self.ptr.as_ptr()
    }
    pub(crate) fn set_preempt(&mut self, preempt: *const AtomicU32) {
        // SAFETY: the owning context is exclusively borrowed and no
        // handler runs on it; raw field writes keep C's provenance.
        unsafe { (&raw mut (*self.as_ptr()).preempt).write(preempt) };
    }
    /// Point the block at `base` and `cards` with `budget`, before a call.
    pub(crate) fn enter(&mut self, base: *mut u8, cards: *mut u8, budget: u64) {
        // SAFETY: as in `set_preempt`.
        unsafe {
            (&raw mut (*self.as_ptr()).base).write(base);
            (&raw mut (*self.as_ptr()).cards).write(cards);
            (&raw mut (*self.as_ptr()).budget).write(budget);
        }
    }
    pub(crate) fn set_budget(&mut self, budget: u64) {
        // SAFETY: as in `set_preempt`; a callback runs with C suspended.
        unsafe { (&raw mut (*self.as_ptr()).budget).write(budget) };
    }
    /// The accesses counted since the last take, cleared.
    pub(crate) fn take_accesses(&mut self) -> u64 {
        // SAFETY: as in `set_budget`.
        unsafe {
            let field = &raw mut (*self.as_ptr()).accesses;
            let count = field.read();
            field.write(0);
            count
        }
    }
}

// SAFETY: the block moves with its context and is used only under the
// context's exclusive borrow; C may not retain it past an invocation.
unsafe impl Send for MemBlock {}
// SAFETY: as above; no shared method touches the block.
unsafe impl Sync for MemBlock {}

impl Drop for MemBlock {
    fn drop(&mut self) {
        // SAFETY: the one owner, made by Box::into_raw; a context is never
        // dropped during a handler, so nothing uses the pointer after this.
        unsafe { drop(Box::from_raw(self.as_ptr())) };
    }
}
