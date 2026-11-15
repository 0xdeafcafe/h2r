//! Optional generated-C RAM path. The atomic gate excludes C accesses from
//! Rust memory guards; C never holds it while calling a Rust helper.
use crate::{ExecCtx, Guest};
use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut};
use std::sync::{
    Mutex, MutexGuard,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

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
        let mut spins = 0;
        while self
            .shared
            .gate
            .compare_exchange_weak(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
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
    pub(crate) fn allows_execute(&self, addr: u32) -> bool {
        if !self.enabled.load(Ordering::Acquire)
            || self
                .shared
                .gate
                .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
        {
            return false;
        }
        // SAFETY: gate excludes both Rust mutations and C operations. A
        // non-null table contains one entry for every 32-bit virtual page.
        let allowed = unsafe {
            let data = &*self.shared.data.get();
            !data.pages.is_null() && (*data.pages.add((addr >> 12) as usize)).permissions & 4 != 0
        };
        self.shared.gate.store(0, Ordering::Release);
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
    pub(crate) accesses: u64,
    pub(crate) budget: u64,
    preempt: *const AtomicU32,
}
// SAFETY: pointers refer to allocations owned by this context's Runtime and
// preemption Arc. Fields are used only under the context's exclusive borrow;
// foreign code may neither retain the view nor use it concurrently.
unsafe impl Send for FastAccess {}
unsafe impl Sync for FastAccess {}

pub(crate) fn sync_accesses(ctx: &mut ExecCtx) {
    let count = ctx
        .fast_memory
        .as_mut()
        .map_or(0, |f| std::mem::take(&mut f.accesses));
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
        Box::new(FastAccess {
            memory,
            accesses: 0,
            budget,
            preempt,
        })
    });
    view.memory = memory;
    view.budget = budget;
    &mut **view
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
