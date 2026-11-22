//! Direct calls between generated handlers (RUNTIME-CONTRACT.md §4,
//! "Direct calls").
//!
//! A module built with `H2_DIRECT_CALLS` (module feature
//! [`FEATURE_DIRECT_CALLS`](crate::generated::FEATURE_DIRECT_CALLS), which
//! needs [`FEATURE_ENTRY_TABLE`](crate::generated::FEATURE_ENTRY_TABLE)) may
//! invoke the handler of a function entry it calls (or tail-jumps to)
//! itself, as a C function, instead of returning `CALL` to the runner. It
//! does that only where the runner would have done exactly the same thing,
//! and two things tell it so:
//!
//! - [`Live`], the runtime's entry table: one handler pointer per entry row
//!   of the module (index = row + 1). It holds the row's generated handler
//!   while dispatch resolves the entry to it (no enabled override) and its
//!   address is lifted code ([`Guest::guard_code`]) with the execute bit,
//!   the host-call thunk ([`h2rt_thunk`]) while an enabled override takes
//!   the entry, and 0 otherwise. [`Runtime::edit_dispatch`] rewrites the
//!   overridden elements before it returns, so an override registered,
//!   enabled or disabled sends calls of that entry to the thunk, or back to
//!   the generated handler, at once. The
//!   execute bit is read when the table is checked against the guest's
//!   code identity ([`Guest::code_id`]), which moves only when lifted code
//!   can have gained or lost it: `map`, `unmap` and `protect_range` refuse
//!   to take Execute from a guarded page, so no per-call read is needed.
//! - The context's block ([`DirectState`], `H2Direct` in h2gen.h), behind
//!   the context's second word (null disables direct calls for the
//!   context). The runner owns it:
//!   - `live`, `pcmap`, `text_lo`, `text_hi`: the entry table and the
//!     module's map from a byte of its text to the entry index there, for
//!     indirect calls.
//!   - `thunk`: [`h2rt_thunk`], which an indirect call or jump outside
//!     `[text_lo, text_hi)` invokes.
//!   - `budget`: what the runner's `max_transfers` has left. Every direct
//!     CALL, JUMP and inline RETURN takes one, as the runner would have.
//!   - `depth`/`cap`: C nesting, which uses host stack. `frames[..depth]`
//!     are the direct CALL/JUMPs live now; after a hand-up the runner
//!     replays them into the shadow stack.
//!   - `view`: the inline RAM view a direct caller lends the handler it is
//!     invoking, so that handler skips `h2abi_v1_fast_acquire` and the
//!     release (module feature
//!     [`FEATURE_DIRECT_VIEW`](crate::generated::FEATURE_DIRECT_VIEW)). The
//!     callee's entry clears it; [`crate::ffi`] clears it before every
//!     native invocation from Rust.
//!
//! The quiet test is the preempt word alone: the runtime keeps
//! [`IRQ_QUEUED`](crate::preempt::IRQ_QUEUED) on it while the interrupt
//! queue is non-empty ([`Runtime::enable_leave_word`], turned on when the
//! entry table is registered).
//!
//! Host calls in place (module feature
//! [`FEATURE_HOST_CALLS`](crate::generated::FEATURE_HOST_CALLS);
//! docs/runtime/contract-thin-transfers-flat-ram.md, section 4.2). The
//! thunk has the handler signature and is called as any direct callee is,
//! after the caller's quiet test, its transfer taken and its frame
//! recorded. It does what the runner's loop would do for that transfer
//! after a safe point that found nothing to do ([`crate::runner::step`]:
//! continuation and kernel tokens, resolution, the execute check, the
//! handler, stop recovery) and returns the handler's transfer. The caller
//! continues inline only on a plain return to its continuation with the
//! word still zero; anything else (a blocking call's STOP, a callback's
//! CALL, a raised preemption) it hands up to the runner as before. Rust
//! code the thunk runs sees what it would under the runner: the live
//! direct frames are in the shadow stack (replayed on entry), and nesting
//! under it starts above them (`floor`), so the depth cap still bounds the
//! host stack. On the way out, if a caller will continue inline (the test
//! `h2_direct_back` makes) the shadow stack is rewound and the frames stay
//! in C for a later hand-up to replay; otherwise the result is handed up
//! past them, the shadow stack stays as the runner would have it, and
//! `depth` drops to the floor so nothing is replayed twice.
//!
//! [`Guest::guard_code`]: crate::Guest::guard_code
//! [`Guest::code_id`]: crate::Guest::code_id
//! [`Runtime::edit_dispatch`]: crate::Runtime::edit_dispatch
//! [`Runtime::enable_leave_word`]: crate::Runtime::enable_leave_word
use crate::runner::{Diag, Step, step};
use crate::{CpuAbi, Event, ExecCtx, ExecCtxOpaque, GuestFn, TransferAbi, abi, dispatch::Dispatch};
use rustc_hash::FxHashMap as HashMap;
use std::ptr::NonNull;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// The deepest nesting the block has room to record.
pub const MAX_DEPTH: u32 = 64;
/// The default nesting cap: each level is one generated handler's host
/// frame on the guest thread's host stack.
pub const DEFAULT_DEPTH: u32 = 32;

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
    /// [`Live::table`], or null until the runtime has an entry table.
    live: *const AtomicUsize,
    /// The entry-table module's `pcmap`, covering `[text_lo, text_hi)`.
    pcmap: *const u16,
    /// [`h2rt_thunk`], set with `live`.
    thunk: Option<GuestFn>,
    text_lo: u32,
    text_hi: u32,
    budget: u64,
    depth: u32,
    cap: u32,
    /// A hold on the C RAM gate lent with `view`, or handed back to a
    /// direct caller (h2gen.h `H2_DIRECT_GATE`); 0 outside generated code.
    gate: u32,
    _pad: u32,
    frames: [Frame; MAX_DEPTH as usize],
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
    assert!(size_of::<Frame>() == 16);
    assert!(size_of::<DirectState>() == 56 + 16 * 64 + 16);
};

/// The rows of the module registered with an entry table
/// ([`FEATURE_ENTRY_TABLE`](crate::generated::FEATURE_ENTRY_TABLE)), as
/// registration checked them. Row `r` has entry index `r + 1`.
pub(crate) struct EntryTable {
    addrs: Box<[u32]>,
    /// The row's handler, when the row is a function entry (kind 0); an
    /// interior entry's element of [`Live`] stays 0.
    functions: Box<[Option<GuestFn>]>,
    index: HashMap<u32, u32>,
    pcmap: *const u16,
    text: (u32, u32),
}

// SAFETY: `pcmap` points into the registered module's read-only data, which
// stays loaded while it is registered (register_generated_module's
// contract); nothing writes through it.
#[allow(unsafe_code)]
unsafe impl Send for EntryTable {}
#[allow(unsafe_code)]
unsafe impl Sync for EntryTable {}

impl EntryTable {
    /// `rows`: each row's address and its handler if it is a function
    /// entry, in table order. `pcmap` and `text` as the module exports them.
    pub(crate) fn new(rows: Vec<(u32, Option<GuestFn>)>, pcmap: *const u16, text: (u32, u32)) -> Self {
        let index = rows
            .iter()
            .enumerate()
            .map(|(i, (a, _))| (*a, i as u32 + 1))
            .collect();
        let (addrs, functions): (Vec<_>, Vec<_>) = rows.into_iter().unzip();
        Self {
            addrs: addrs.into_boxed_slice(),
            functions: functions.into_boxed_slice(),
            index,
            pcmap,
            text,
        }
    }
    /// Entry `addr`'s index, if it is a row.
    pub(crate) fn index_of(&self, addr: u32) -> Option<u32> {
        self.index.get(&addr).copied()
    }
    pub(crate) fn len(&self) -> usize {
        self.addrs.len()
    }
}

/// The runtime's entry table; see the module doc.
pub(crate) struct Live {
    rows: std::sync::Arc<EntryTable>,
    /// `live` in h2gen.h: element `i` for entry index `i`, 0 for none.
    table: Box<[AtomicUsize]>,
    /// The [`Guest::code_id`](crate::Guest::code_id) the table was last
    /// checked under (0: never).
    checked: AtomicU64,
    state: Mutex<LiveState>,
}

#[derive(Default)]
struct LiveState {
    /// The code identity `executable` was read under.
    code: u64,
    /// Per row: lifted code with the execute bit, under `code`.
    executable: Vec<bool>,
    /// Indices zeroed for an enabled override, sorted.
    overridden: Vec<u32>,
}

impl Live {
    pub(crate) fn new(rows: std::sync::Arc<EntryTable>) -> Self {
        let table = (0..=rows.len()).map(|_| AtomicUsize::new(0)).collect();
        Self {
            rows,
            table,
            checked: AtomicU64::new(0),
            state: Mutex::default(),
        }
    }
    pub(crate) fn rows(&self) -> &EntryTable {
        &self.rows
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, LiveState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// The code identity the table was last checked under.
    #[inline]
    pub(crate) fn checked(&self) -> u64 {
        self.checked.load(Ordering::Acquire)
    }
    /// What element `i` holds while overridden: the host-call thunk.
    fn overridden_value() -> usize {
        h2rt_thunk as GuestFn as usize
    }
    /// What element `i` holds when not overridden.
    fn value(&self, st: &LiveState, i: u32) -> usize {
        let row = i as usize - 1;
        match self.rows.functions[row] {
            Some(f) if st.executable.get(row).copied().unwrap_or(false) => f as usize,
            _ => 0,
        }
    }
    /// The indices of rows with an enabled override in `d`, sorted.
    fn overridden_in(&self, d: &Dispatch) -> Vec<u32> {
        let mut v: Vec<u32> = d
            .enabled_overrides()
            .filter_map(|a| self.rows.index_of(a))
            .collect();
        v.sort_unstable();
        v
    }
    /// The guest's lifted code read `executable` (per row) under `code`:
    /// rewrite every element.
    pub(crate) fn check_code(&self, code: u64, executable: Vec<bool>, d: &Dispatch) {
        let mut st = self.lock();
        if st.code != code {
            st.code = code;
            st.executable = executable;
            st.overridden = self.overridden_in(d);
            for i in 1..=self.rows.len() as u32 {
                let v = if st.overridden.binary_search(&i).is_ok() {
                    Self::overridden_value()
                } else {
                    self.value(&st, i)
                };
                self.table[i as usize].store(v, Ordering::Release);
            }
        }
        self.checked.store(code, Ordering::Release);
    }
    /// Dispatch was edited: bring the overridden elements in step. Called
    /// under the dispatch write lock, before the edit returns.
    pub(crate) fn sync_overrides(&self, d: &Dispatch) {
        let mut st = self.lock();
        let now = self.overridden_in(d);
        for &i in &st.overridden {
            if now.binary_search(&i).is_err() {
                self.table[i as usize].store(self.value(&st, i), Ordering::Release);
            }
        }
        for &i in &now {
            self.table[i as usize].store(Self::overridden_value(), Ordering::Release);
        }
        st.overridden = now;
    }
    /// Element `i` (0 for an index out of range).
    pub(crate) fn get(&self, i: u32) -> usize {
        self.table
            .get(i as usize)
            .map_or(0, |e| e.load(Ordering::Acquire))
    }
    /// Each row's address, in table order.
    pub(crate) fn addrs(&self) -> &[u32] {
        &self.rows.addrs
    }
}

/// The context's own block and the pointer generated code reads (the
/// block, or null while disabled).
pub(crate) struct Direct {
    owned: NonNull<DirectState>,
    /// The frames below this are a host-call thunk's caller's, already in
    /// the shadow stack: a runner inside the thunk replays and drops only
    /// those above it. 0 outside a thunk.
    floor: u32,
}

/// `ExecCtx`'s second field: generated code reads
/// `*(H2Direct **)((char *)ctx + sizeof(void *))`.
#[repr(transparent)]
pub(crate) struct DirectPtr(*mut DirectState);

// SAFETY: both point into the block the same ExecCtx owns, which moves with
// it; only the thread running the context touches the block, and the
// pointers inside it target the entry table of the Arc<Runtime> the context
// holds and the registered module's read-only pcmap.
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
        // SAFETY: all-zero bytes are a valid DirectState (null pointers,
        // zero counts).
        let mut state: Box<DirectState> = unsafe { Box::new_zeroed().assume_init() };
        state.cap = DEFAULT_DEPTH;
        let owned = NonNull::from(Box::leak(state));
        let mut d = Self { owned, floor: 0 };
        if let Some(live) = runtime.live() {
            d.install(live);
        }
        d
    }

    /// Point the block at the runtime's entry table.
    fn install(&mut self, live: &Live) {
        let s = self.state();
        s.live = live.table.as_ptr();
        s.pcmap = live.rows.pcmap;
        s.thunk = Some(h2rt_thunk as GuestFn);
        (s.text_lo, s.text_hi) = live.rows.text;
    }

    /// Reset invocation state, keeping the entry table.
    pub(crate) fn restart(&mut self) {
        self.floor = 0;
        let s = self.state();
        s.budget = 0;
        s.depth = 0;
        s.cap = DEFAULT_DEPTH;
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
    /// The direct frames live right now and not yet in the shadow stack:
    /// while generated code runs (an event's backtrace), every level of the
    /// C nesting above the floor; after it handed a transfer up, the levels
    /// it left.
    fn frames(&self) -> &[Frame] {
        // SAFETY: as `state`.
        let s = unsafe { self.owned.as_ref() };
        let depth = (s.depth as usize).min(MAX_DEPTH as usize);
        &s.frames[(self.floor as usize).min(depth)..depth]
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

/// The host-call thunk (`H2Direct.thunk`, h2gen.h; see the module doc):
/// the handler a direct call invokes for an overridden entry and an
/// indirect call or jump invokes for a target outside the module's text.
/// The caller recorded the transfer as its top direct frame (its kind and
/// continuation) and took its transfer from the budget.
///
/// # Safety
/// Called only by generated code, with the live, exclusive pair it was
/// invoked with, as it invokes any handler (the handler ABI, ffi.rs), from
/// `h2_direct_enter`'s side of a direct call.
pub(crate) unsafe extern "C" fn h2rt_thunk(
    cpu: *mut CpuAbi,
    exec: *mut ExecCtxOpaque,
    target: u32,
) -> TransferAbi {
    // SAFETY: the caller's contract; generated code touches neither until
    // this returns.
    let (cpu, ctx) = unsafe { (&mut *cpu, &mut *exec.cast::<ExecCtx>()) };
    if crate::runner::diagnostics(ctx) {
        host_call::<true>(cpu, ctx, target)
    } else {
        host_call::<false>(cpu, ctx, target)
    }
}

fn host_call<const DIAG: bool>(cpu: &mut CpuAbi, ctx: &mut ExecCtx, target: u32) -> TransferAbi {
    ctx.direct_take_lent();
    // The runner's checks before its safe point; the caller's quiet test
    // was the safe point.
    if let Some(id) = ctx.pending_fault.take() {
        return TransferAbi::fault(id);
    }
    if DIAG && !cpu.valid_abi() {
        return TransferAbi::fault(ctx.emit(Event::InvalidAbi));
    }
    let floor = ctx.direct.floor;
    let (depth, top) = {
        let s = ctx.direct.state();
        let depth = s.depth.min(MAX_DEPTH);
        (depth, depth.checked_sub(1).map(|i| s.frames[i as usize]))
    };
    let transfer = match top {
        Some(f) if depth > floor && f.target == target && f.kind == abi::CALL => {
            TransferAbi::call(target, f.resume)
        }
        Some(f) if depth > floor && f.target == target && f.kind == abi::JUMP => TransferAbi::jump(target),
        _ => return TransferAbi::fault(ctx.emit(Event::InvalidTransfer)),
    };
    // The shadow stack as the runner would have it when it routed this
    // transfer: the frames above the floor, this one last.
    let mark = ctx.frames.mark();
    replay(&mut ctx.frames, ctx.direct.frames());
    ctx.direct.floor = depth;
    // The caller took this transfer's one; step takes it as the runner does.
    let mut remaining = ctx.direct.state().budget.saturating_add(1);
    let mut diag = Diag::thunk::<DIAG>(ctx);
    let out = match step::<false, DIAG, true>(cpu, ctx, transfer, (0, 0), &mut remaining, &mut diag) {
        Step::Next(t) | Step::Done(t) => t,
    };
    // Will a caller above the floor continue inline with `out`? The
    // generated callers make the same test (h2_direct_back): a JUMP's
    // caller hands up, a CALL's continues on a plain return to its
    // continuation, ESP above the slot, the word zero and a transfer left.
    let caught = remaining != 0
        && ctx.leave_word() == 0
        && out.kind == abi::RETURN
        && out.route == abi::RETURN_TARGET
        && out.event_id == 0
        && ctx.direct.state().frames[floor as usize..depth as usize]
            .iter()
            .any(|f| f.kind == abi::CALL && f.resume == out.target_pc && cpu.esp > f.slot);
    let s = ctx.direct.state();
    s.budget = remaining;
    // Nothing lent back to the caller.
    s.view = std::ptr::null_mut();
    s.gate = 0;
    s.acc = 0;
    if caught {
        // The frames stay live in C and are replayed if a later transfer
        // is handed up; what this transfer recorded is popped by the
        // return, as the runner's unwind would have popped it.
        s.depth = depth;
        ctx.frames.rewind(mark);
    } else {
        // Handed up to whatever runs below the floor: the shadow stack is
        // already what the runner would have, so nothing is left to
        // replay.
        s.depth = floor;
    }
    ctx.direct.floor = floor;
    out
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
    /// left. The entry table is checked again first if the guest's lifted
    /// code may have changed since ([`crate::Runtime::live_checked`]).
    #[inline]
    pub(crate) fn direct_begin(&mut self, remaining: u64) {
        let live = self.runtime.live_checked();
        let s = self.direct.state();
        s.budget = remaining;
        if s.live.is_null()
            && let Some(live) = live
        {
            self.direct.install(live);
        }
    }
    /// The invocation returned: what is left of `remaining`. Transfers the
    /// handlers made directly count as routed ones (`count`; not inside the
    /// host-call thunk, whose caller's runner counts them), and frames they
    /// left live enter the shadow stack as the runner would have recorded
    /// them.
    #[inline]
    pub(crate) fn direct_end(&mut self, remaining: u64, count: bool) -> u64 {
        let left = self.direct.state().budget.min(remaining);
        let used = remaining - left;
        if count
            && used != 0
            && let Some(p) = &mut self.profile
        {
            p.transfers += used;
            p.direct_transfers += used;
        }
        if !self.direct.frames().is_empty() {
            replay(&mut self.frames, self.direct.frames());
            let floor = self.direct.floor;
            self.direct.state().depth = floor;
        }
        left
    }
    /// The host-call thunk was entered: what the generated caller lent with
    /// the call belongs to a generated callee, so settle it here. The view
    /// is the context's own; its count and a lent count are spent as the
    /// caller's exit would have spent them before the runner ran this
    /// transfer, and a lent hold on the RAM gate is released.
    fn direct_take_lent(&mut self) {
        let s = self.direct.state();
        let view = std::mem::replace(&mut s.view, std::ptr::null_mut());
        let gate = std::mem::take(&mut s.gate);
        let acc = std::mem::take(&mut s.acc);
        if gate != 0 {
            // SAFETY: the view the caller lent with the hold, which it gave
            // up (h2gen.h H2_LEND_); it takes back only what `gate` holds,
            // and that is 0 when the thunk returns.
            unsafe { crate::ffi::release_lent_gate(view) };
        }
        crate::ffi::sync_accesses(self);
        if acc != 0 {
            self.spend_accesses(acc);
        }
    }
    /// The shadow stack with the live direct frames on top.
    pub(crate) fn backtrace_with_direct(&self, pc: u32) -> Vec<crate::GuestFrame> {
        match self.direct.frames() {
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
    /// What the entry table holds for entry `pc` (`H2_ENTRY_STATS`).
    pub(crate) fn live_state(&self, pc: u32) -> crate::entry_stats::Slot {
        use crate::entry_stats::Slot as S;
        let Some(live) = self.runtime().live() else {
            return S::NoEntry;
        };
        match live.rows().index_of(pc) {
            None => S::NoEntry,
            Some(i) if live.get(i) != 0 => S::Live,
            Some(_) => S::Null,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn a(_: *mut CpuAbi, _: *mut ExecCtxOpaque, _: u32) -> TransferAbi {
        TransferAbi::stop(0)
    }
    unsafe extern "C" fn b(_: *mut CpuAbi, _: *mut ExecCtxOpaque, _: u32) -> TransferAbi {
        TransferAbi::stop(1)
    }

    fn table() -> Live {
        // Function A at 0x1000, an interior label at 0x1010, function B at
        // 0x2000.
        Live::new(std::sync::Arc::new(EntryTable::new(
            vec![(0x1000, Some(a as GuestFn)), (0x1010, None), (0x2000, Some(b as GuestFn))],
            std::ptr::null(),
            (0x1000, 0x2001),
        )))
    }

    #[test]
    fn indices_are_rows_plus_one_and_zero_is_none() {
        let live = table();
        assert_eq!(live.rows().index_of(0x1000), Some(1));
        assert_eq!(live.rows().index_of(0x1010), Some(2));
        assert_eq!(live.rows().index_of(0x2000), Some(3));
        assert_eq!(live.rows().index_of(0x1001), None);
        assert_eq!(live.table.len(), 4);
        assert_eq!(live.get(0), 0);
        assert_eq!(live.get(99), 0);
    }

    #[test]
    fn nothing_is_live_until_the_code_is_checked() {
        let live = table();
        live.sync_overrides(&Dispatch::default());
        assert_eq!((live.get(1), live.get(2), live.get(3)), (0, 0, 0));
        assert_eq!(live.checked(), 0);
    }

    #[test]
    fn a_checked_table_holds_executable_function_entries_only() {
        let live = table();
        live.check_code(7, vec![true, true, false], &Dispatch::default());
        assert_eq!(live.checked(), 7);
        assert_eq!(live.get(1), a as *const () as usize);
        assert_eq!(live.get(2), 0, "an interior entry is never live");
        assert_eq!(live.get(3), 0, "not lifted executable code");
        // The same identity again changes nothing; a new one re-reads.
        live.check_code(7, vec![false, false, true], &Dispatch::default());
        assert_eq!((live.get(1), live.get(3)), (a as *const () as usize, 0));
        live.check_code(8, vec![false, false, true], &Dispatch::default());
        assert_eq!((live.get(1), live.get(3)), (0, b as *const () as usize));
    }

    #[test]
    fn overrides_take_the_thunk_and_give_the_handler_back() {
        let thunk = h2rt_thunk as GuestFn as usize;
        let live = table();
        let mut d = Dispatch::default();
        live.check_code(1, vec![true, true, true], &d);
        d.register_override(0x2000, "o", |_, _, _| Ok(TransferAbi::stop(2)));
        live.sync_overrides(&d);
        assert_eq!((live.get(1), live.get(3)), (a as *const () as usize, thunk));
        d.set_enabled(0x2000, false);
        live.sync_overrides(&d);
        assert_eq!(live.get(3), b as *const () as usize);
        d.set_enabled(0x2000, true);
        live.sync_overrides(&d);
        // A re-check keeps an override's element on the thunk.
        live.check_code(2, vec![true, true, true], &d);
        assert_eq!((live.get(1), live.get(3)), (a as *const () as usize, thunk));
        // An override off the table touches nothing.
        d.register_override(0x1001, "elsewhere", |_, _, _| Ok(TransferAbi::stop(3)));
        live.sync_overrides(&d);
        assert_eq!(live.get(1), a as *const () as usize);
    }
}
