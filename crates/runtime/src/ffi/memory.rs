//! Optional generated-C RAM path. The atomic gate excludes C accesses from
//! Rust memory guards; C never holds it while calling a Rust helper.
use crate::{ExecCtx, Guest};
use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;
use std::sync::{
    Mutex, MutexGuard,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

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
}
impl Memory {
    pub(crate) fn new(guest: Guest) -> Self {
        Self {
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
        let guard = self.contention.lock(&self.guest);
        if !self.enabled.load(Ordering::Acquire) {
            return MemoryGuard {
                guard,
                shared: None,
                ready: &self.ready,
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
            ready: &self.ready,
        };
        // Publish every C store to subscribers before exposing Guest to HLE.
        // SAFETY: result owns the gate and Guest mutex until its Drop.
        let data = unsafe { &mut *self.shared.data.get() };
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
    pub(crate) fn fast_shared(&self) -> *const MemoryShared {
        if !self.ready.load(Ordering::Acquire) {
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
        &*self.shared
    }
}

/// A compound guest-memory borrow. Like MutexGuard, it cannot cross threads.
/// Its additional gate synchronizes inline generated-C loads and stores.
pub struct MemoryGuard<'a> {
    guard: MutexGuard<'a, Guest>,
    shared: Option<&'a MemoryShared>,
    ready: &'a AtomicBool,
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
        let Some(shared) = self.shared else {
            return;
        };
        let (ram, pages, watching) = self.guard.fast_parts();
        // SAFETY: this guard still owns the gate. Re-derive the pointers after
        // every Rust borrow, including replacement of the entire Guest.
        let data = unsafe { &mut *shared.data.get() };
        data.ram = ram;
        data.pages = pages;
        data.watching = u32::from(watching);
        self.ready.store(!pages.is_null(), Ordering::Release);
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
    let count = ctx
        .fast_memory
        .as_mut()
        .map_or(0, OwnedFastAccess::take_accesses);
    if count != 0 {
        if let Some(p) = &mut ctx.profile {
            p.fast_memory_accesses += count;
        }
        ctx.spend_accesses(count);
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
    assert!(size_of::<MemoryData>() == 2080);
    assert!(size_of::<MemoryShared>() == 2088);
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
