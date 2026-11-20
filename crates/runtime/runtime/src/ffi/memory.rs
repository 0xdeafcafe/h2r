//! Optional generated-C RAM path. The atomic gate excludes C accesses from
//! Rust memory guards; C never holds it while calling a Rust helper.
//!
//! The memory also owns the flat RAM window ([`crate::window`]) and, for
//! the flat model, the state word that replaces the gate (contract change
//! section 4.1, "No gate in generated code"):
//!
//! - **Idle**: no generated code runs on this memory and no guard is held.
//! - **Locked**: a guard taken while idle is held.
//! - **Running**: a context's generated code runs ([`call_native`]); the
//!   word carries that context's preempt word.
//! - **In Rust**: that thread's helper is taking or holds a guard.
//!
//! A guard on another thread while the state is running sets [`PARK`] on
//! the running context's word and waits, holding the Guest mutex, until
//! the state leaves running. Generated code reaches a safe point within one
//! loop trip or one call and returns to the runner, which takes the bit
//! like any wake bit (a park is not a preemption: no slice ends and no
//! transfer is spent) and waits at its next entry until the guard is gone.
//! A helper on the running thread sets "in Rust" before it blocks on the
//! Guest mutex and restores "running" when its guard drops, so a guard
//! waiting meanwhile goes first. A Rust helper must not wait for another
//! thread's guard while its own thread is running generated code without
//! taking a guard itself: that thread never reaches a safe point.
//!
//! The state word is used only once [`crate::Runtime::enable_parking`] has
//! been called: the flat model (slice F3) has no gate. On the paged path
//! the gate already excludes generated accesses from guards, and no guard
//! ever sets [`PARK`].
//!
//! [`call_native`]: crate::ffi::call_native
//! [`PARK`]: crate::preempt::PARK
use crate::{ExecCtx, Guest};
use std::cell::{Cell, UnsafeCell};
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
};

/// The state word: idle, locked, or a context's preempt word with a tag.
const IDLE: usize = 0;
const LOCKED: usize = 1;
const TAG: usize = 3;
const RUNNING: usize = 2;
const IN_RUST: usize = 3;
/// A guard with no state to put back when it drops.
const NO_RESTORE: usize = usize::MAX;

thread_local! {
    /// The memory (its address) whose generated code this thread runs.
    static RUNS: Cell<usize> = const { Cell::new(0) };
}

/// Spin, then yield, as the gate's waiter does.
fn back_off(spins: &mut u32) {
    if *spins < 64 {
        std::hint::spin_loop();
        *spins += 1;
    } else {
        std::thread::yield_now();
    }
}

/// Gate bits, as in `h2fastmem.h`: the holder, and a Rust guard waiting.
const GATE_HELD: u32 = 1;
const GATE_WANTED: u32 = 2;

#[repr(C)]
pub struct MemoryData {
    pages: *const crate::guest::FastPage,
    ram: *mut u8,
    dirty: [u64; 256],
    dirty_lo: u32,
    dirty_hi: u32,
    watching: u32,
    #[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")))]
    flat: crate::flat::View,
}
impl Default for MemoryData {
    fn default() -> Self {
        Self {
            pages: std::ptr::null(),
            ram: std::ptr::null_mut(),
            dirty: [0; 256],
            dirty_lo: 256,
            dirty_hi: 0,
            watching: 0,
            #[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")))]
            flat: Default::default(),
        }
    }
}
#[repr(C)]
pub struct MemoryShared {
    gate: AtomicU32,
    data: UnsafeCell<MemoryData>,
}
// SAFETY: data and the pointed-to Guest allocations are accessed only while
// holding gate. Rust also holds the Guest mutex; C only tries the gate and
// falls back after releasing it. The owning Memory outlives every C view.
unsafe impl Send for MemoryShared {}
unsafe impl Sync for MemoryShared {}

pub(crate) struct Memory {
    guest: Mutex<Guest>,
    contention: crate::services::LockContention,
    shared: Box<MemoryShared>,
    ready: AtomicBool,
    enabled: AtomicBool,
    /// The Guest's [`Guest::layout_id`] as the last guard left it.
    layout: AtomicU64,
    /// The Guest's [`Guest::code_id`] as the last guard left it.
    code: AtomicU64,
    /// The flat RAM window, for the memory's whole life, where there is one.
    window: Option<Arc<crate::window::Window>>,
    /// The state word (module docs); used once `parking` is on.
    state: AtomicUsize,
    parking: AtomicBool,
    /// A flat-RAM module runs on this memory ([`Memory::make_flat`]).
    flat: AtomicBool,
    /// Guards waiting for generated code to park: it may not start again
    /// until they have had the Guest.
    waiters: AtomicUsize,
    /// Every context's preempt word ([`crate::Runtime`] shares it), to set
    /// [`crate::preempt::PARK`] on the running one.
    pub(crate) words: Arc<crate::preempt::Words>,
}
impl Memory {
    pub(crate) fn new(mut guest: Guest) -> Self {
        let window = crate::window::Window::new();
        if let Some(w) = &window {
            guest.attach_window(w.clone());
        }
        Self {
            layout: AtomicU64::new(guest.layout_id()),
            code: AtomicU64::new(guest.code_id()),
            window,
            state: AtomicUsize::new(IDLE),
            parking: AtomicBool::new(false),
            flat: AtomicBool::new(false),
            waiters: AtomicUsize::new(0),
            words: Default::default(),
            guest: Mutex::new(guest),
            contention: Default::default(),
            shared: Box::new(MemoryShared {
                gate: AtomicU32::new(0),
                data: UnsafeCell::new(MemoryData::default()),
            }),
            ready: AtomicBool::new(false),
            enabled: AtomicBool::new(false),
        }
    }
    pub(crate) fn contention(&self) -> crate::services::Contention {
        self.contention.snapshot()
    }
    pub(crate) fn lock(&self) -> MemoryGuard<'_> {
        let parking = self.parking.load(Ordering::Relaxed);
        let mut restore = if parking { self.leave_running() } else { NO_RESTORE };
        let guard = self.contention.lock(&self.guest);
        if parking && restore == NO_RESTORE {
            restore = self.wait_for_park();
        }
        if !self.enabled.load(Ordering::Acquire) {
            return MemoryGuard {
                guard,
                shared: None,
                memory: self,
                restore,
            };
        }
        // C holds the gate across runs of accesses and retakes it at once,
        // so a waiter says so: C takes only a clear gate and releases its
        // own bit alone, which leaves this guard next in line.
        let gate = &self.shared.gate;
        let mut spins = 0;
        loop {
            let seen = gate.load(Ordering::Relaxed);
            if seen & GATE_HELD == 0 {
                if gate
                    .compare_exchange_weak(seen, GATE_HELD, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    break;
                }
                continue;
            }
            if seen & GATE_WANTED == 0 {
                gate.fetch_or(GATE_WANTED, Ordering::Relaxed);
            }
            if spins < 64 {
                std::hint::spin_loop();
                spins += 1;
            } else {
                std::thread::yield_now();
            }
        }
        let mut result = MemoryGuard {
            guard,
            shared: Some(&self.shared),
            memory: self,
            restore,
        };
        // Publish every C store to subscribers before exposing Guest to HLE.
        // SAFETY: result owns the gate and Guest mutex until its Drop.
        let data = unsafe { &mut *self.shared.data.get() };
        #[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")))]
        result.guard.publish_flat();
        for word in data.dirty_lo..data.dirty_hi {
            let mut bits = std::mem::take(&mut data.dirty[word as usize]);
            while bits != 0 {
                let bit = bits.trailing_zeros() as usize;
                result.guard.note_fast_page(word as usize * 64 + bit);
                bits &= bits - 1;
            }
        }
        data.dirty_lo = 256;
        data.dirty_hi = 0;
        result
    }
    /// Take the gate without the Guest mutex, if it is entirely clear.
    fn try_gate(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
            && self
                .shared
                .gate
                .compare_exchange(0, GATE_HELD, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
    }
    /// Release a gate taken by [`Self::try_gate`]; the word it leaves. Only
    /// the held bit is cleared: a guard may have started waiting meanwhile,
    /// and a plain store of 0 would erase its request.
    fn release_gate(&self) -> u32 {
        self.shared.gate.fetch_and(!GATE_HELD, Ordering::Release) & !GATE_HELD
    }
    pub(crate) fn allows_execute(&self, addr: u32) -> bool {
        if !self.try_gate() {
            return false;
        }
        // SAFETY: gate excludes both Rust mutations and C operations. A
        // non-null table contains one entry for every 32-bit virtual page.
        let allowed = unsafe {
            let data = &*self.shared.data.get();
            !data.pages.is_null() && (*data.pages.add((addr >> 12) as usize)).permissions & 4 != 0
        };
        self.release_gate();
        allowed
    }
    /// The guest layout every guard so far has left ([`Guest::layout_id`]).
    /// A guard publishes its Guest's layout as it ends, before it lets go
    /// of the gate and the Guest: whoever reads a layout here reads it after
    /// the changes that made it, and a guard still changing the layout has
    /// not published yet (an execute check made meanwhile, under the old
    /// value, is ordered before that guard).
    #[inline]
    pub(crate) fn layout(&self) -> u64 {
        self.layout.load(Ordering::Acquire)
    }
    /// The lifted-code identity every guard so far has left
    /// ([`Guest::code_id`]), published as [`Memory::layout`] is.
    #[inline]
    pub(crate) fn code(&self) -> u64 {
        self.code.load(Ordering::Acquire)
    }
    /// Every generated handler's entry asks (`h2abi_v1_fast_acquire`):
    /// the usual answer, a ready view, is one load inline.
    #[inline]
    pub(crate) fn fast_shared(&self) -> *const MemoryShared {
        if !self.ready.load(Ordering::Acquire) {
            self.prepare_fast_shared();
        }
        &*self.shared
    }
    /// The flat RAM window, where there is one.
    pub(crate) fn window(&self) -> Option<&Arc<crate::window::Window>> {
        self.window.as_ref()
    }
    /// A flat-RAM module runs on this memory: its Guest, and any Guest put
    /// in its place, keeps RAM in a backing the window can alias
    /// ([`Guest::require_aliasable_ram`]). Takes the Guest.
    pub(crate) fn make_flat(&self) {
        if self.flat.swap(true, Ordering::AcqRel) {
            return;
        }
        self.lock().require_aliasable_ram();
    }
    /// Use the state word from now on (module docs).
    pub(crate) fn enable_parking(&self) {
        self.parking.store(true, Ordering::Release);
    }
    #[inline]
    pub(crate) fn parking(&self) -> bool {
        self.parking.load(Ordering::Relaxed)
    }
    fn me(&self) -> usize {
        self as *const Self as usize
    }
    /// Before a guard blocks on the Guest mutex: on the thread running
    /// this memory's generated code, go "in Rust", and give the running
    /// state to put back when the guard drops.
    #[cold]
    #[inline(never)]
    fn leave_running(&self) -> usize {
        if RUNS.with(Cell::get) != self.me() {
            return NO_RESTORE;
        }
        let s = self.state.load(Ordering::Acquire);
        if s & TAG != RUNNING || s == LOCKED {
            return NO_RESTORE;
        }
        self.state.store((s & !TAG) | IN_RUST, Ordering::Release);
        s
    }
    /// With the Guest mutex held, on any other thread: wait until no
    /// generated code runs, asking the running context to park. Returns the
    /// state to put back when the guard drops.
    #[cold]
    #[inline(never)]
    fn wait_for_park(&self) -> usize {
        self.waiters.fetch_add(1, Ordering::AcqRel);
        let mut spins = 0;
        let mut asked = IDLE;
        let restore = loop {
            let s = self.state.load(Ordering::Acquire);
            if s == IDLE {
                if self
                    .state
                    .compare_exchange(IDLE, LOCKED, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
                {
                    break IDLE;
                }
                continue;
            }
            if s != LOCKED && s & TAG == IN_RUST {
                // Its helper waits for this mutex and puts "running" back
                // only once it has had it.
                break NO_RESTORE;
            }
            if s != LOCKED && s & TAG == RUNNING && s != asked {
                self.words.request_on(s & !TAG, crate::preempt::PARK);
                asked = s;
            } else if s & TAG != RUNNING {
                asked = IDLE;
            }
            back_off(&mut spins);
        };
        self.waiters.fetch_sub(1, Ordering::AcqRel);
        restore
    }
    /// [`crate::ffi::call_native`]'s entry: mark this thread as running
    /// generated code with `word`, once no guard holds or waits for the
    /// Guest. Returns what [`Memory::stop_running`] puts back.
    #[cold]
    #[inline(never)]
    pub(crate) fn start_running(&self, word: *const AtomicU32) -> (usize, usize) {
        let running = word as usize | RUNNING;
        let outer = RUNS.with(|r| r.replace(self.me()));
        if outer == self.me() {
            // Generated code entered again from a helper on this thread.
            return (self.state.swap(running, Ordering::AcqRel), outer);
        }
        let mut spins = 0;
        let mut asked = IDLE;
        loop {
            if self.waiters.load(Ordering::Acquire) == 0
                && self
                    .state
                    .compare_exchange(IDLE, running, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
            {
                return (IDLE, outer);
            }
            // Another thread's generated code: one runs at a time.
            let s = self.state.load(Ordering::Acquire);
            if s != LOCKED && s & TAG == RUNNING && s != asked {
                self.words.request_on(s & !TAG, crate::preempt::PARK);
                asked = s;
            } else if s & TAG != RUNNING {
                asked = IDLE;
            }
            back_off(&mut spins);
        }
    }
    /// [`crate::ffi::call_native`]'s exit.
    pub(crate) fn stop_running(&self, (state, outer): (usize, usize)) {
        self.state.store(state, Ordering::Release);
        RUNS.with(|r| r.set(outer));
    }
    #[cold]
    #[inline(never)]
    fn prepare_fast_shared(&self) {
        let mut guard = self.lock();
        if guard.shared.is_none() {
            // No C view exists yet. Enable while holding the Guest mutex,
            // before publishing a view or allowing any ungated Rust borrow.
            self.shared.gate.store(1, Ordering::Relaxed);
            guard.shared = Some(&self.shared);
            self.enabled.store(true, Ordering::Release);
        }
        guard.prepare_fast_pages();
    }
}

/// A compound guest-memory borrow. Like MutexGuard, it cannot cross threads.
/// Its additional gate synchronizes inline generated-C loads and stores.
pub struct MemoryGuard<'a> {
    guard: MutexGuard<'a, Guest>,
    shared: Option<&'a MemoryShared>,
    memory: &'a Memory,
    /// The state word to put back as the guard drops, or [`NO_RESTORE`].
    restore: usize,
}
impl Deref for MemoryGuard<'_> {
    type Target = Guest;
    fn deref(&self) -> &Guest {
        &self.guard
    }
}
impl DerefMut for MemoryGuard<'_> {
    fn deref_mut(&mut self) -> &mut Guest {
        &mut self.guard
    }
}
impl Drop for MemoryGuard<'_> {
    fn drop(&mut self) {
        let memory = self.memory;
        // A Guest put in place of another is shown in the window from now,
        // over RAM the window can alias if a flat-RAM module runs.
        if let Some(w) = &memory.window
            && !self.guard.shown_in(w)
        {
            if memory.flat.load(Ordering::Acquire) {
                self.guard.require_aliasable_ram();
            }
            self.guard.attach_window(w.clone());
        }
        // Still holding the Guest (and the gate, if any): see Memory::layout.
        memory.layout.store(self.guard.layout_id(), Ordering::Release);
        memory.code.store(self.guard.code_id(), Ordering::Release);
        if let Some(shared) = self.shared {
            self.publish(shared);
        }
        // Last, still holding the Guest mutex: see Memory::wait_for_park.
        if self.restore != NO_RESTORE {
            memory.state.store(self.restore, Ordering::Release);
        }
    }
}
impl MemoryGuard<'_> {
    fn publish(&mut self, shared: &MemoryShared) {
        #[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")))]
        let flat = self.guard.flat_view();
        let (ram, pages, watching) = self.guard.fast_parts();
        // SAFETY: this guard still owns the gate. Re-derive the pointers after
        // every Rust borrow, including replacement of the entire Guest.
        let data = unsafe { &mut *shared.data.get() };
        data.ram = ram;
        data.pages = pages;
        data.watching = u32::from(watching);
        #[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")))]
        if let Some(flat) = flat {
            data.flat = flat;
            data.watching |= 1 << 31;
            if flat.stores.is_null() && !flat.cold_stores.is_null() {
                data.watching |= 1 << 30;
            }
        }
        self.memory.ready.store(!pages.is_null(), Ordering::Release);
        shared.gate.store(0, Ordering::Release);
    }
}

/// Invocation-scoped context view, separate from architectural ABI layouts.
#[repr(C)]
pub struct FastAccess {
    memory: *const MemoryShared,
    accesses: u64,
    budget: u64,
    preempt: *const AtomicU32,
}
// SAFETY: pointers refer to allocations owned by this context's Runtime and
// preemption Arc. Fields are used only under the context's exclusive borrow;
// foreign code may neither retain the view nor use it concurrently.
unsafe impl Send for FastAccess {}
unsafe impl Sync for FastAccess {}

/// Owns the allocation without reborrowing it through Box while generated C
/// retains its pointer. All accesses (including callbacks) derive from `ptr`.
/// Box is reconstructed only at destruction, after the invocation has ended.
pub(crate) struct OwnedFastAccess {
    ptr: NonNull<FastAccess>,
}
impl OwnedFastAccess {
    fn new(view: FastAccess) -> Self {
        Self {
            ptr: NonNull::new(Box::into_raw(Box::new(view))).expect("Box is non-null"),
        }
    }
    fn as_ptr(&self) -> *mut FastAccess {
        self.ptr.as_ptr()
    }
    fn take_accesses(&mut self) -> u64 {
        // SAFETY: the context is exclusively borrowed during a callback; C is
        // suspended. Raw field accesses preserve the retained pointer's provenance.
        unsafe {
            let field = &raw mut (*self.as_ptr()).accesses;
            let count = field.read();
            field.write(0);
            count
        }
    }
    pub(crate) fn set_budget(&mut self, budget: u64) {
        // SAFETY: called only by the exclusively borrowed context, with C suspended.
        unsafe {
            (&raw mut (*self.as_ptr()).budget).write(budget);
        }
    }
    fn configure(&mut self, memory: *const MemoryShared, budget: u64) {
        // SAFETY: same exclusive invocation contract. Never re-create an owning
        // Box or a reference to the allocation while a foreign pointer is live.
        unsafe {
            (&raw mut (*self.as_ptr()).memory).write(memory);
        }
        self.set_budget(budget);
    }
}
// SAFETY: ownership may move with ExecCtx; use is serialized by its exclusive
// invocation borrow. The view itself is not exposed by safe shared methods.
unsafe impl Send for OwnedFastAccess {}
unsafe impl Sync for OwnedFastAccess {}
impl Drop for OwnedFastAccess {
    fn drop(&mut self) {
        // SAFETY: exactly one owner, created by Box::into_raw. ExecCtx cannot be
        // dropped during a borrowed native invocation; retained pointers may not
        // escape that invocation. Nothing accesses the pointer after this drop.
        unsafe {
            drop(Box::from_raw(self.as_ptr()));
        }
    }
}

pub(crate) fn sync_accesses(ctx: &mut ExecCtx) {
    // A flat-RAM handler counts in the context's block, a paged one in its
    // view; only one of them is ever non-zero.
    let count = ctx
        .fast_memory
        .as_mut()
        .map_or(0, OwnedFastAccess::take_accesses)
        + ctx.mem_block.take_accesses();
    if count != 0 {
        if let Some(p) = &mut ctx.profile {
            p.fast_memory_accesses += count;
        }
        ctx.spend_accesses(count);
    }
}

/// A hold on the C RAM gate that a direct caller lent with `view`
/// (h2gen.h `H2_DIRECT_GATE`) to a callee that is not generated code (the
/// host-call thunk, direct.rs), which gives nothing back: release it as
/// C's `H2_GATE_RELEASE` does, clearing only the holder's bit.
///
/// # Safety
/// `view` is the context's own view, lent by the generated caller that
/// holds the gate, and the caller does not use the hold again.
pub(crate) unsafe fn release_lent_gate(view: *mut FastAccess) {
    if view.is_null() {
        return;
    }
    // SAFETY: the caller's contract; raw reads keep C's provenance.
    unsafe {
        let memory = (&raw const (*view).memory).read();
        if let Some(m) = memory.as_ref() {
            m.gate.fetch_and(!GATE_HELD, Ordering::Release);
        }
    }
}

/// Acquire the optional RAM view. This does not hold the gate across calls.
///
/// # Safety
/// `exec` is the current exclusive context. The returned view is valid only
/// until the handler returns. C must follow h2fastmem.h's gate, dirty-bit and
/// access-budget protocol and release the gate before invoking any helper.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2abi_v1_fast_acquire(exec: *mut crate::ExecCtxOpaque) -> *mut FastAccess {
    // SAFETY: the invocation-scoped exclusive context is the caller contract.
    let ctx = unsafe { &mut *exec.cast::<ExecCtx>() };
    sync_accesses(ctx);
    if !ctx.fast_memory_enabled {
        return std::ptr::null_mut();
    }
    let memory = ctx.runtime().fast_memory();
    let budget = if ctx.slice.armed && ctx.slice.left != 0 {
        ctx.slice.left
    } else {
        u64::MAX
    };
    let preempt = ctx.preempt_ptr();
    let view = ctx.fast_memory.get_or_insert_with(|| {
        OwnedFastAccess::new(FastAccess {
            memory,
            accesses: 0,
            budget,
            preempt,
        })
    });
    view.configure(memory, budget);
    view.as_ptr()
}

/// Settle inline access counts at every generated exit, including faults.
///
/// # Safety
/// `exec` is the live exclusive context, with no C gate held.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2abi_v1_fast_release(exec: *mut crate::ExecCtxOpaque) {
    // SAFETY: the caller supplies the invocation-scoped context.
    sync_accesses(unsafe { &mut *exec.cast::<ExecCtx>() });
}

#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(std::mem::offset_of!(MemoryData, watching) == 2072);
    #[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")))]
    {
        assert!(std::mem::offset_of!(MemoryData, flat) == 2080);
        assert!(size_of::<crate::flat::View>() == 40);
        assert!(std::mem::offset_of!(crate::flat::View, cold_stores) == 24);
        assert!(size_of::<MemoryData>() == 2120);
    }
    #[cfg(not(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64"))))]
    assert!(size_of::<MemoryData>() == 2080);
    assert!(std::mem::offset_of!(MemoryShared, data) == 8);
    assert!(size_of::<FastAccess>() == 32);
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guest::Permissions;
    use crate::{CpuAbi, Dispatch, Runtime};

    /// A guard waiting on a gate that C holds marks it wanted. C's release
    /// clears only its own bit, C's try (a clear gate only) then fails, and
    /// the guard gets the gate and leaves it clear.
    #[test]
    fn waiting_guard_marks_the_gate_and_is_next() {
        let rt = Runtime::new(Guest::new(), Dispatch::default());
        rt.memory().map(0x1000, 0x1000, 0, Permissions::RW).unwrap();
        // SAFETY: the view outlives this test's uses; only its gate is touched.
        let gate = unsafe { &(*rt.fast_memory()).gate };
        // What h2_gate_take does.
        assert_eq!(gate.compare_exchange(0, GATE_HELD, Ordering::Acquire, Ordering::Relaxed), Ok(0));
        let host = rt.clone();
        let waiter = std::thread::spawn(move || {
            let mut m = host.memory();
            m.set_u32(0x1000, 7).unwrap();
        });
        while gate.load(Ordering::Relaxed) & GATE_WANTED == 0 {
            std::thread::yield_now();
        }
        assert_eq!(gate.load(Ordering::Relaxed), GATE_HELD | GATE_WANTED);
        // C cannot retake a wanted gate, held or released.
        assert!(gate.compare_exchange(0, GATE_HELD, Ordering::Acquire, Ordering::Relaxed).is_err());
        // What H2_GATE_RELEASE does.
        gate.fetch_and(!GATE_HELD, Ordering::Release);
        waiter.join().unwrap();
        assert_eq!(gate.load(Ordering::Relaxed), 0);
        assert_eq!(rt.memory().u32(0x1000).unwrap(), 7);
    }

    /// A guard that starts waiting while an execute check holds the gate
    /// keeps its request when the check ends, and is next.
    #[test]
    fn execute_check_release_keeps_a_waiting_guards_request() {
        let memory = std::sync::Arc::new(Memory::new(Guest::new()));
        memory.lock().map(0x1000, 0x1000, 0, Permissions::RX).unwrap();
        memory.fast_shared();
        assert!(memory.allows_execute(0x1000));
        assert_eq!(memory.shared.gate.load(Ordering::Relaxed), 0);
        // The execute check, stopped between its take and its release.
        assert!(memory.try_gate());
        let host = memory.clone();
        let waiter = std::thread::spawn(move || {
            host.lock().protect(0x1000, Permissions::READ).unwrap();
        });
        while memory.shared.gate.load(Ordering::Relaxed) & GATE_WANTED == 0 {
            std::thread::yield_now();
        }
        // No other check or C access can cut in ahead of the waiter.
        assert!(!memory.try_gate());
        assert_eq!(memory.release_gate(), GATE_WANTED);
        waiter.join().unwrap();
        assert_eq!(memory.shared.gate.load(Ordering::Relaxed), 0);
        assert!(!memory.allows_execute(0x1000));
    }

    /// `h2abi_v1_exit(n)` leaves a context exactly as the pair it replaces
    /// did at a handler's exit, `h2abi_v1_charge(n)` (only when n is not 0)
    /// then `h2abi_v1_fast_release`: for every slice (none, host requests
    /// only, about to run out, run out), count the inline view holds and
    /// count the handler settles, the same charged and inline totals, slice
    /// spent, left and expiry, preempt word and view budget and count.
    #[test]
    fn exit_settles_as_charge_then_release() {
        let rt = Runtime::new(Guest::new(), Dispatch::default());
        let context = |slice: Option<u64>, held: u64| {
            let mut ctx = ExecCtx::new(rt.clone(), 0);
            ctx.enable_profiling(true);
            if let Some(n) = slice {
                ctx.arm_preemption(n);
            }
            // As h2abi_v1_fast_acquire configures it (no RAM: count only).
            let budget = if ctx.slice.armed && ctx.slice.left != 0 { ctx.slice.left } else { u64::MAX };
            ctx.fast_memory = Some(OwnedFastAccess::new(FastAccess {
                memory: std::ptr::null(),
                accesses: held,
                budget,
                preempt: ctx.preempt_ptr(),
            }));
            ctx
        };
        let state = |ctx: &mut ExecCtx| {
            let view = ctx.fast_memory.as_ref().unwrap().as_ptr();
            // SAFETY: the view is the context's own, not in use elsewhere.
            let (accesses, budget) = unsafe { ((*view).accesses, (*view).budget) };
            let p = ctx.take_profile().unwrap();
            (
                (p.memory_accessors, p.fast_memory_accesses),
                (ctx.slice.armed, ctx.slice.left, ctx.slice.expired, ctx.slice.spent),
                ctx.preempt_pending(),
                (accesses, budget),
            )
        };
        let mut cases = 0;
        for slice in [None, Some(0), Some(1), Some(2), Some(3), Some(5), Some(8)] {
            for held in 0..6u64 {
                for n in 0..6u64 {
                    let mut pair = context(slice, held);
                    let mut exit = context(slice, held);
                    let p = (&mut pair as *mut ExecCtx).cast();
                    let e = (&mut exit as *mut ExecCtx).cast();
                    // SAFETY: each context is live and used by nothing else.
                    unsafe {
                        if n != 0 {
                            crate::ffi::h2abi_v1_charge(p, n);
                        }
                        h2abi_v1_fast_release(p);
                        crate::ffi::h2abi_v1_exit(e, n);
                    }
                    assert_eq!(state(&mut exit), state(&mut pair), "slice {slice:?}, view {held}, handler {n}");
                    cases += 1;
                }
            }
        }
        assert_eq!(cases, 7 * 6 * 6);
    }

    /// Rust-only model of a native handler retaining its actual runtime view
    /// across real slow callbacks. Does not load C, so it also runs under Miri.
    #[test]
    fn retained_fast_view_survives_slow_callbacks() {
        let rt = Runtime::new(Guest::new(), Dispatch::default());
        {
            let mut memory = rt.memory();
            memory.map(0x1000, 0x1000, 0, Permissions::RW).unwrap();
            memory.map(0x2000, 0x1000, 0x4000, Permissions::RW).unwrap();
            memory.write_bytes(0x1fff, &[1, 2, 3, 4], 1).unwrap();
        }
        let mut ctx = ExecCtx::new(rt, 0);
        ctx.enable_profiling(true);
        ctx.arm_preemption(5);
        // Only the count/budget fields are used by this Rust model of C. Avoid
        // allocating the million-entry C page table in the interpreter; the
        // allocation owner and slow callback paths below are production code.
        ctx.fast_memory = Some(OwnedFastAccess::new(FastAccess {
            memory: std::ptr::null(),
            accesses: 0,
            budget: 5,
            preempt: ctx.preempt_ptr(),
        }));
        let retained = ctx.fast_memory.as_ref().unwrap().as_ptr();
        let exec = (&mut ctx as *mut ExecCtx).cast();
        let mut cpu = CpuAbi::default();
        let mut bytes = [0; 4];
        let mut old = 0;
        // SAFETY: mimic the exclusively running C handler. Every Rust callback
        // completes before that same raw pointer is reused; buffers are disjoint.
        unsafe {
            (*retained).accesses += 1;
            assert_eq!(
                crate::ffi::h2abi_v1_read(exec, 0x8000, 0x1fff, 4, 1, bytes.as_mut_ptr()),
                0
            );
            assert_eq!(bytes, [1, 2, 3, 4]);
            assert_eq!((*retained).accesses, 0);
            assert_eq!((*retained).budget, 3);
            (*retained).accesses += 1;
            assert_eq!(
                crate::ffi::h2abi_v1_xchg(&mut cpu, exec, 0x8001, 0x1000, 4, 42, &mut old),
                0
            );
            assert_eq!(old, 0);
            assert_eq!((*retained).budget, 1);
            (*retained).accesses += 1;
            h2abi_v1_fast_release(exec);
            assert_eq!((*retained).accesses, 0);
            assert_eq!((*retained).budget, u64::MAX);
        }
        assert_eq!(ctx.slice_spent(), 5);
        assert_ne!(ctx.preempt_pending(), 0);
        let profile = ctx.take_profile().unwrap();
        assert_eq!(
            (profile.memory_accessors, profile.fast_memory_accesses),
            (5, 3)
        );
        assert_eq!(ctx.memory().u32(0x1000).unwrap(), 42);
    }
}

#[cfg(all(test, not(miri), any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64"))))]
mod flat_tests;

#[cfg(all(test, not(miri)))]
mod park_tests;
