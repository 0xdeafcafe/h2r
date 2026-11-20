//! Direct calls between generated handlers (RUNTIME-CONTRACT.md §4,
//! "Direct calls").
//!
//! A module built with `H2_DIRECT_CALLS` (module feature
//! [`FEATURE_DIRECT_CALLS`](crate::generated::FEATURE_DIRECT_CALLS)) may
//! invoke the handler of a function entry it calls (or tail-jumps to)
//! itself, as a C function, instead of returning `CALL` to the runner. It
//! does that only where the runner would have done exactly the same thing,
//! and this block is how it knows. The context's second word points to it
//! (null disables direct calls for the context). The runner owns it:
//!
//! - `slots`: entry `pc` resolved to native handler `fn` (selector `pc`) at
//!   dispatch generation `generation`. Filled by the runner whenever it
//!   dispatches an entry route to a native handler; C compares the
//!   generation with the runtime's live one, so an [`edit_dispatch`]
//!   (an override registered, enabled or removed) stops every direct call
//!   at once until the runner has resolved the target again.
//! - `interrupts`: the runtime's queued interrupt count. With it zero and the
//!   preempt word zero the runner's safe point does nothing.
//! - `budget`: what the runner's `max_transfers` has left. Every direct
//!   CALL, JUMP and inline RETURN takes one, as the runner would have.
//! - `depth`/`cap`: C nesting, which uses host stack.
//! - `frames`: direct CALL/JUMPs whose handler handed a transfer up without
//!   returning inline; the runner replays them into the shadow stack.
//! - `view`: the inline RAM view a direct caller lends the handler it is
//!   invoking, so that handler skips `h2abi_v1_fast_acquire` and the
//!   release (module feature
//!   [`FEATURE_DIRECT_VIEW`](crate::generated::FEATURE_DIRECT_VIEW)). The
//!   callee's entry clears it; [`crate::ffi`] clears it before every native
//!   invocation from Rust.
//!
//! [`edit_dispatch`]: crate::Runtime::edit_dispatch
use crate::{ExecCtx, GuestFn, abi, dispatch::Handler};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, AtomicUsize};

/// Room in the per-context table; `h2_direct_slot` in h2gen.h hashes the
/// same way, into the first `slot_mask + 1`.
pub(crate) const SLOTS: usize = 8192;
/// Slots in use: a power of two up to [`SLOTS`] from `H2_DIRECT_SLOTS`
/// (runtime environment), else 4096.
fn slots_in_use() -> u32 {
    static N: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("H2_DIRECT_SLOTS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|n| n.is_power_of_two() && *n as usize <= SLOTS)
            .unwrap_or(4096)
    })
}
/// The deepest nesting the block has room to record.
pub const MAX_DEPTH: u32 = 64;
/// The default nesting cap: each level is one generated handler's host
/// frame on the guest thread's host stack.
pub const DEFAULT_DEPTH: u32 = 32;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Slot {
    generation: u64,
    handler: Option<GuestFn>,
    pc: u32,
    _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Frame {
    kind: u32,
    target: u32,
    resume: u32,
    slot: u32,
}

/// `H2Direct` in h2gen.h.
#[repr(C)]
pub(crate) struct DirectState {
    generation: *const AtomicU64,
    interrupts: *const AtomicUsize,
    budget: u64,
    depth: u32,
    cap: u32,
    frames_len: u32,
    /// A hold on the C RAM gate lent with `view`, or handed back to a
    /// direct caller (h2gen.h `H2_DIRECT_GATE`); 0 outside generated code.
    gate: u32,
    frames: [Frame; MAX_DEPTH as usize],
    slot_mask: u32,
    _pad2: u32,
    slots: [Slot; SLOTS],
    /// The inline RAM view a direct caller lends its callee (module feature
    /// [`FEATURE_DIRECT_VIEW`](crate::generated::FEATURE_DIRECT_VIEW)):
    /// set just before the call, taken and cleared by the callee's entry.
    view: *mut crate::ffi::FastAccess,
    /// The uncharged access count lent with `view`, or handed back to a
    /// direct caller (h2gen.h `H2_DIRECT_COUNT`); 0 outside generated code.
    acc: u64,
}

#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(size_of::<Slot>() == 24 && size_of::<Frame>() == 16);
    assert!(size_of::<DirectState>() == 40 + 16 * 64 + 8 + 24 * 8192 + 16);
};

/// `h2_direct_slot` in h2gen.h.
fn slot_of(pc: u32, mask: u32) -> usize {
    ((pc ^ (pc >> 8)) & mask) as usize
}

/// The context's own block and the pointer generated code reads (the
/// block, or null while disabled).
pub(crate) struct Direct {
    owned: NonNull<DirectState>,
}

/// `ExecCtx`'s second field: generated code reads
/// `*(H2Direct **)((char *)ctx + sizeof(void *))`.
#[repr(transparent)]
pub(crate) struct DirectPtr(*mut DirectState);

// SAFETY: both point into the block the same ExecCtx owns, which moves with
// it; only the thread running the context touches the block, and the two
// pointers inside it target atomics of the Arc<Runtime> the context holds.
#[allow(unsafe_code)]
unsafe impl Send for Direct {}
#[allow(unsafe_code)]
unsafe impl Sync for Direct {}
#[allow(unsafe_code)]
unsafe impl Send for DirectPtr {}
#[allow(unsafe_code)]
unsafe impl Sync for DirectPtr {}

impl Drop for Direct {
    fn drop(&mut self) {
        // SAFETY: allocated by Box::new in `new`, freed only here.
        drop(unsafe { Box::from_raw(self.owned.as_ptr()) });
    }
}

impl Direct {
    pub(crate) fn new(runtime: &crate::Runtime) -> Self {
        // About 200 KiB: built in place, all zeros (every field's empty
        // value: null pointers, None handlers), not on the stack.
        // SAFETY: all-zero bytes are a valid DirectState.
        let mut state: Box<DirectState> = unsafe { Box::new_zeroed().assume_init() };
        state.generation = runtime.dispatch_generation_ptr();
        state.interrupts = runtime.interrupts.pending_ptr();
        state.cap = DEFAULT_DEPTH;
        state.slot_mask = slots_in_use() - 1;
        let owned = NonNull::from(Box::leak(state));
        Self { owned }
    }

    /// Reset invocation state while keeping the warmed entries. Their
    /// generation and execute checks still run on each use.
    pub(crate) fn restart(&mut self) {
        let s = self.state();
        s.budget = 0;
        s.depth = 0;
        s.cap = DEFAULT_DEPTH;
        s.frames_len = 0;
        s.gate = 0;
        s.view = std::ptr::null_mut();
        s.acc = 0;
    }

    pub(crate) fn as_ptr(&self) -> DirectPtr {
        DirectPtr(self.owned.as_ptr())
    }

    fn state(&mut self) -> &mut DirectState {
        // SAFETY: owned by self; generated code writes it only while it runs
        // with this context, never concurrently with a Rust borrow.
        unsafe { self.owned.as_mut() }
    }
    /// The direct frames live right now: while generated code runs (an
    /// event's backtrace), every level of the C nesting.
    fn live(&self) -> &[Frame] {
        // SAFETY: as `state`.
        let s = unsafe { self.owned.as_ref() };
        &s.frames[..(s.frames_len as usize).min(MAX_DEPTH as usize)]
    }
}

/// Record `frames` in `stack` as the runner records the transfers.
fn replay(stack: &mut crate::frames::ShadowStack, frames: &[Frame]) {
    for f in frames {
        stack.unwind(f.slot);
        if f.kind == abi::CALL {
            stack.call(f.target, f.resume, f.slot);
        }
        stack.enter(f.target);
    }
}

impl ExecCtx {
    /// Let generated code of a direct-call module invoke its callees
    /// directly on this context (the default), or make every call go
    /// through the runner, as a module built with `H2_DIRECT_CALLS=0` does.
    /// For comparisons and diagnostics; the result of a run is the same.
    pub fn enable_direct_calls(&mut self, enabled: bool) {
        self.direct_ptr.0 = if enabled {
            self.direct.owned.as_ptr()
        } else {
            std::ptr::null_mut()
        };
    }
    /// Cap direct-call nesting at `depth` (at most [`MAX_DEPTH`]; default
    /// [`DEFAULT_DEPTH`]). A call past it goes through the runner.
    pub fn set_direct_call_depth(&mut self, depth: u32) {
        self.direct.state().cap = depth.min(MAX_DEPTH);
    }
    /// About to invoke a native handler from Rust: nothing lends it a view,
    /// whatever a direct call into a handler that does not take one left.
    #[inline]
    pub(crate) fn direct_clear_view(&mut self) {
        let s = self.direct.state();
        s.view = std::ptr::null_mut();
        s.gate = 0;
        s.acc = 0;
    }
    /// The runner is about to invoke a handler with `remaining` transfers
    /// left.
    #[inline]
    pub(crate) fn direct_begin(&mut self, remaining: u64) {
        self.direct.state().budget = remaining;
    }
    /// The invocation returned: what is left of `remaining`. Transfers the
    /// handlers made directly count as routed ones, and frames they left
    /// live enter the shadow stack as the runner would have recorded them.
    #[inline]
    pub(crate) fn direct_end(&mut self, remaining: u64) -> u64 {
        let left = self.direct.state().budget.min(remaining);
        let used = remaining - left;
        if used != 0
            && let Some(p) = &mut self.profile
        {
            p.transfers += used;
            p.direct_transfers += used;
        }
        if !self.direct.live().is_empty() {
            replay(&mut self.frames, self.direct.live());
            let s = self.direct.state();
            s.frames_len = 0;
            s.depth = 0;
        }
        left
    }
    /// The shadow stack with the live direct frames on top.
    pub(crate) fn backtrace_with_direct(&self, pc: u32) -> Vec<crate::GuestFrame> {
        match self.direct.live() {
            [] => self.frames.snapshot(pc),
            live => {
                let mut stack = self.frames.clone();
                replay(&mut stack, live);
                stack.snapshot(pc)
            }
        }
    }
    /// The budget a nested runner must give back to the one it runs inside.
    pub(crate) fn direct_budget(&mut self) -> u64 {
        self.direct.state().budget
    }
    pub(crate) fn set_direct_budget(&mut self, budget: u64) {
        self.direct.state().budget = budget;
    }
    /// What entry `pc`'s slot holds for `handler` at dispatch `generation`
    /// (`H2_ENTRY_STATS`).
    pub(crate) fn direct_slot_state(&self, pc: u32, handler: &Handler, generation: u64) -> crate::entry_stats::Slot {
        use crate::entry_stats::Slot as S;
        // SAFETY: as `state`; read only, between invocations.
        let d = unsafe { self.direct.owned.as_ref() };
        let s = d.slots[slot_of(pc, d.slot_mask)];
        let f = match handler {
            Handler::Native(f) => Some(*f),
            _ => None,
        };
        if s.handler.is_none() && s.pc == 0 {
            S::Empty
        } else if s.pc != pc {
            S::Collision
        } else if s.generation == generation && s.handler.map(|h| h as usize) == f.map(|h| h as usize) {
            S::Hit
        } else {
            S::Stale
        }
    }
    /// The runner resolved entry `pc` to `handler` with selector `pc` at
    /// dispatch `generation`.
    #[inline]
    pub(crate) fn note_direct(&mut self, pc: u32, handler: &Handler, generation: u64) {
        if let Handler::Native(f) = handler {
            let s = self.direct.state();
            let slot = &mut s.slots[slot_of(pc, s.slot_mask)];
            if slot.generation != generation || slot.pc != pc {
                *slot = Slot {
                    generation,
                    handler: Some(*f),
                    pc,
                    _pad: 0,
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slots_hash_as_the_header_does() {
        // h2gen.h: (pc ^ (pc >> 8)) & d->slot_mask
        for pc in [0u32, 0x1234_5678, 0xffff_ffff, 0x0040_0100] {
            assert_eq!(slot_of(pc, 255), ((pc ^ (pc >> 8)) & 255) as usize);
            assert_eq!(slot_of(pc, 4095), ((pc ^ (pc >> 8)) & 4095) as usize);
        }
        assert!(slots_in_use().is_power_of_two() && slots_in_use() as usize <= SLOTS);
    }
}
