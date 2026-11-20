//! Protect only aliases used by generated stores. A 16 KiB host uses four
//! mirrors: physical guest page p is written through mirror (p & 3), so
//! opening one host page never admits stores to an unreported sibling.
//! Host/HLE and legacy images continue to write the ordinary RAM mapping.
use super::{PAGE, RAM_SIZE, Ram, Reservation};
use crate::watch::WriteWatch;
use std::mem::MaybeUninit;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering::*};
use std::sync::{Arc, OnceLock};

const WORDS: usize = RAM_SIZE / PAGE / 64;
#[cfg(target_os = "macos")]
const SIGNAL: i32 = libc::SIGBUS;
#[cfg(target_os = "linux")]
const SIGNAL: i32 = libc::SIGSEGV;

// Process totals include mirrors retired by restore or guest destruction.
// Only the two relaxed increments in fault() can execute in a signal handler.
static FAULTS: AtomicU64 = AtomicU64::new(0);
static RETRIES: AtomicU64 = AtomicU64::new(0);
static REPROTECTS: AtomicU64 = AtomicU64::new(0);
static ARMED: AtomicU64 = AtomicU64::new(0);
static PROMOTED: AtomicU64 = AtomicU64::new(0);
static STATS: OnceLock<bool> = OnceLock::new();

extern "C" fn report_stats() {
    use std::io::Write;
    let _ = writeln!(
        std::io::stderr().lock(),
        "flat RAM: faults={} retries={} re-protects={} pages-armed={} software-pages={}",
        FAULTS.load(Relaxed),
        RETRIES.load(Relaxed),
        REPROTECTS.load(Relaxed),
        ARMED.load(Relaxed),
        PROMOTED.load(Relaxed)
    );
}

pub(super) fn enable_stats() -> bool {
    *STATS.get_or_init(|| {
        std::env::var_os("H2_FLAT_STATS").is_some_and(|v| v == "1")
            && unsafe { libc::atexit(report_stats) } == 0
    })
}

struct State {
    stats: bool,
    base: usize,
    host: usize,
    open: [AtomicU64; WORDS],
    pending: [AtomicU64; WORDS],
    any: AtomicBool,
}
impl State {
    fn page_address(&self, p: usize) -> usize {
        let offset = p * PAGE;
        self.base + (p & (self.host / PAGE - 1)) * RAM_SIZE + (offset & !(self.host - 1))
    }
    fn fault(&self, address: usize) -> bool {
        let offset = address.wrapping_sub(self.base);
        let p = offset % RAM_SIZE / PAGE;
        if offset / RAM_SIZE != (p & (self.host / PAGE - 1)) {
            return false;
        }
        let bit = 1 << (p % 64);
        if self.open[p / 64].load(Relaxed) & bit != 0 {
            return false;
        }
        // No allocation, logging or locks here. The writer holds the memory
        // gate. Only this kind of data-write fault may open this exact alias.
        if self.stats {
            FAULTS.fetch_add(1, Relaxed);
        }
        let res = unsafe {
            libc::mprotect(
                self.page_address(p) as *mut _,
                self.host,
                libc::PROT_READ | libc::PROT_WRITE,
            )
        };
        if res != 0 {
            return false;
        }
        self.open[p / 64].fetch_or(bit, Relaxed);
        self.pending[p / 64].fetch_or(bit, Relaxed);
        self.any.store(true, Release);
        if self.stats {
            RETRIES.fetch_add(1, Relaxed);
        }
        true
    }
}

// Slot storage and the previously installed action live for the process.
// A reader announces before fetching pointers; removal unpublishes before
// waiting. Even a simultaneous unrelated host fault cannot see freed state.
static REGIONS: [AtomicPtr<State>; 16] = [const { AtomicPtr::new(ptr::null_mut()) }; 16];
static READERS: AtomicUsize = AtomicUsize::new(0);
static PREVIOUS: OnceLock<libc::sigaction> = OnceLock::new();

fn install() -> bool {
    static INSTALLED: OnceLock<bool> = OnceLock::new();
    *INSTALLED.get_or_init(|| unsafe {
        let mut action = MaybeUninit::zeroed();
        if libc::sigaction(SIGNAL, ptr::null(), action.as_mut_ptr()) != 0 {
            return false;
        }
        let mut action: libc::sigaction = action.assume_init();
        // An automatically reset action cannot be chained persistently.
        if action.sa_flags & libc::SA_RESETHAND != 0 {
            return false;
        }
        if PREVIOUS.set(action).is_err() {
            return false;
        }
        action.sa_sigaction = handler as *const () as usize;
        action.sa_flags |= libc::SA_SIGINFO;
        // Retain the original mask, alternate stack and restart behaviour.
        libc::sigaction(SIGNAL, &action, ptr::null_mut()) == 0
    })
}

unsafe fn data_write(context: *mut libc::c_void) -> bool {
    let uc = unsafe { &*context.cast::<libc::ucontext_t>() };
    #[cfg(target_os = "linux")]
    {
        let regs = &uc.uc_mcontext.gregs;
        regs[libc::REG_TRAPNO as usize] == 14 && matches!(regs[libc::REG_ERR as usize], 6 | 7)
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        let esr = unsafe { (*uc.uc_mcontext).__es.__esr };
        esr >> 26 == 0x24 && esr & (1 << 6) != 0 && esr & (1 << 8) == 0
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        let es = unsafe { &(*uc.uc_mcontext).__es };
        es.__trapno == 14 && matches!(es.__err, 6 | 7)
    }
}

extern "C" fn handler(sig: i32, info: *mut libc::siginfo_t, context: *mut libc::c_void) {
    // Darwin reports SIGBUS with BUS_ADRALN for a protection fault.
    // Check the saved machine exception as well as the reported address.
    let code = unsafe { (*info).si_code };
    if (1..=3).contains(&code) && unsafe { data_write(context) } {
        let address = unsafe { (*info).si_addr() } as usize;
        READERS.fetch_add(1, SeqCst);
        let handled = REGIONS.iter().any(|slot| {
            let p = slot.load(SeqCst);
            !p.is_null() && unsafe { (*p).fault(address) }
        });
        READERS.fetch_sub(1, SeqCst);
        if handled {
            return;
        }
    }
    // Published before sigaction; never changed. Forward the original
    // siginfo and mutable saved context to a prior three-argument action.
    let Some(old) = PREVIOUS.get() else {
        unsafe { libc::_exit(128 + sig) }
    };
    unsafe {
        match old.sa_sigaction {
            libc::SIG_IGN => {}
            libc::SIG_DFL => {
                if libc::sigaction(sig, old, ptr::null_mut()) != 0 || libc::raise(sig) != 0 {
                    libc::_exit(128 + sig);
                }
            }
            f if old.sa_flags & libc::SA_SIGINFO != 0 => {
                std::mem::transmute::<
                    usize,
                    extern "C" fn(i32, *mut libc::siginfo_t, *mut libc::c_void),
                >(f)(sig, info, context);
            }
            f => std::mem::transmute::<usize, extern "C" fn(i32)>(f)(sig),
        }
    }
}

pub(super) struct Mirror {
    state: Arc<State>,
    reservation: Reservation,
    slot: usize,
    epoch: u64,
    // Under the gate: a page faulting in a second protection epoch stays
    // open and uses the ordinary per-store physical dirty bits thereafter.
    seen: Box<[u64; WORDS]>,
    hot: Box<[u64; WORDS]>,
    has_hot: bool,
}
impl Mirror {
    pub(super) fn new(ram: &Ram, host: usize, watch: &WriteWatch) -> Option<Self> {
        let size = RAM_SIZE * (host / PAGE);
        let reservation = Reservation::new(size)?;
        let base = reservation.ptr.as_ptr();
        for off in (0..size).step_by(RAM_SIZE) {
            // SAFETY: destination plane is ours, source is the physical store.
            if !unsafe { ram.alias(base.wrapping_add(off), 0, RAM_SIZE) } {
                return None;
            }
        }
        if !install() {
            return None;
        }
        let state = Arc::new(State {
            stats: enable_stats(),
            base: base as usize,
            host,
            any: AtomicBool::new(false),
            // All pages start writable. Only watched, clean physical
            // pages need even a first-write fault.
            open: std::array::from_fn(|_| AtomicU64::new(u64::MAX)),
            pending: std::array::from_fn(|_| AtomicU64::new(0)),
        });
        let raw = Arc::as_ptr(&state).cast_mut();
        let slot = REGIONS.iter().position(|s| {
            s.compare_exchange(ptr::null_mut(), raw, SeqCst, SeqCst)
                .is_ok()
        })?;
        let mut mirror = Self {
            state,
            reservation,
            slot,
            epoch: watch.epoch.wrapping_sub(1),
            seen: Box::new([0; WORDS]),
            hot: Box::new([0; WORDS]),
            has_hot: false,
        };
        if !mirror.rearm(watch) {
            return None;
        }
        Some(mirror)
    }
    pub(super) fn base(&self) -> *mut u8 {
        self.reservation.ptr.as_ptr()
    }
    pub(super) fn software_pages(&self) -> *const u64 {
        self.hot.as_ptr()
    }
    pub(super) fn has_hot_pages(&self) -> bool {
        self.has_hot
    }

    pub(super) fn publish(&mut self, watch: &mut WriteWatch) {
        if !self.state.any.load(Acquire) {
            return;
        }
        self.state.any.store(false, Relaxed);
        for (w, word) in self.state.pending.iter().enumerate() {
            let mut bits = word.load(Relaxed);
            if bits == 0 {
                continue;
            }
            word.store(0, Relaxed);
            let repeat = bits & self.seen[w] & !self.hot[w];
            if repeat != 0 {
                self.hot[w] |= repeat;
                self.has_hot = true;
                if self.state.stats {
                    PROMOTED.fetch_add(repeat.count_ones().into(), Relaxed);
                }
            }
            self.seen[w] |= bits;
            while bits != 0 {
                watch.note((w * 64 + bits.trailing_zeros() as usize) * PAGE, PAGE);
                bits &= bits - 1;
            }
        }
    }
    // After publication, open pages need no new fault until a subscriber
    // drains or changes its watch. Ordinary host borrows leave them open.
    pub(super) fn rearm(&mut self, watch: &WriteWatch) -> bool {
        if self.epoch == watch.epoch {
            return true;
        }
        self.epoch = watch.epoch;
        // Consecutive physical host pages in each plane share one syscall.
        let mut runs = [None; 4];
        let protect = |(start, end, repeats): (usize, usize, u64)| {
            let ok = unsafe { libc::mprotect(start as *mut _, end - start, libc::PROT_READ) == 0 };
            if ok && self.state.stats {
                ARMED.fetch_add(((end - start) / self.state.host) as u64, Relaxed);
                REPROTECTS.fetch_add(repeats, Relaxed);
            }
            ok
        };
        for (w, word) in self.state.open.iter().enumerate() {
            let eligible = word.load(Relaxed) & !self.hot[w];
            if eligible == 0 {
                continue;
            }
            let mut bits = eligible & watch.awaiting_writes(w);
            let rearmed = bits;
            while bits != 0 {
                let bit = bits & bits.wrapping_neg();
                let p = w * 64 + bits.trailing_zeros() as usize;
                bits &= !bit;
                let at = self.state.page_address(p);
                let run = &mut runs[p & (self.state.host / PAGE - 1)];
                let again = u64::from(self.seen[w] & bit != 0);
                *run = Some(match *run {
                    Some((lo, hi, n)) if hi == at => (lo, at + self.state.host, n + again),
                    previous => {
                        if previous.is_some_and(|r| !protect(r)) {
                            return false;
                        }
                        (at, at + self.state.host, again)
                    }
                });
            }
            if rearmed != 0 {
                word.fetch_and(!rearmed, Relaxed);
            }
        }
        runs.into_iter().flatten().all(protect)
    }
}
impl Drop for Mirror {
    fn drop(&mut self) {
        REGIONS[self.slot].store(ptr::null_mut(), SeqCst);
        while READERS.load(SeqCst) != 0 {
            std::hint::spin_loop();
        }
    }
}
