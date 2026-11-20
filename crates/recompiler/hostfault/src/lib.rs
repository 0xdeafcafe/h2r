//! The process-wide host fault handler (contract change, section 4.1 of
//! docs/runtime/contract-thin-transfers-flat-ram.md).
//!
//! One handler per process, installed on first use: `SIGBUS` and `SIGSEGV`
//! on Apple systems, `SIGSEGV` on Linux. It knows two things, both held in
//! fixed arrays so the handler never allocates or locks:
//!
//! - **Regions**: an address range and a callback that runs inside the
//!   handler for a fault whose data address is in the range
//!   ([`register_region`]). The callback may move the saved PC
//!   ([`Fault::resume_at`]) and say the fault is handled. [`fixup`] is the
//!   callback a flat RAM window uses; another owner (the sim tier's
//!   rollback) brings its own.
//! - **Fixup tables**: rows of (access instruction, landing label) that a
//!   generated module emits beside every inline access
//!   ([`register_table`]). [`lookup`] finds a faulting PC's landing by
//!   binary search. A table must be sorted by instruction address; it is
//!   checked once at registration and never sorted or skipped here.
//!
//! A fault in no region, or one every callback declines, goes to the
//! handler that was installed before this one, unchanged: the Rust
//! runtime's stack-overflow report, a debugger, or the default action.
//!
//! Callbacks run in a signal handler: no allocation, locks, logging or
//! panics. Unregistering ([`Region`] and [`Table`] drops) unpublishes the
//! entry and then waits until no handler is reading, so a fault racing the
//! removal never sees freed state.
//!
//! Windows (a vectored handler) is not here yet: [`register_region`]
//! returns [`Error::Unsupported`] there and tables can still be checked
//! and searched.

use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering::SeqCst};

mod table;
#[cfg(unix)]
mod unix;

pub use table::{Format, MAX_TABLES, Table, lookup, register_table};

/// How many regions may be registered at once.
pub const MAX_REGIONS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// This target has no handler yet.
    Unsupported,
    /// `sigaction` failed, or the previous action resets itself and so
    /// cannot be chained.
    Install,
    /// Every slot is taken.
    Full,
    /// The range is empty or wraps the address space.
    BadRange,
    /// The range overlaps a registered region.
    Overlap,
    /// A fixup table's bounds are reversed, misaligned or not a whole
    /// number of rows.
    BadTable,
    /// Row `index` of a fixup table does not come after the row before it.
    Unsorted { index: usize },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unsupported => {
                f.write_str("host fault handling is not supported on this target")
            }
            Error::Install => f.write_str("the host fault handler could not be installed"),
            Error::Full => f.write_str("no free host fault slot"),
            Error::BadRange => f.write_str("empty or wrapping host fault region"),
            Error::Overlap => f.write_str("host fault region overlaps a registered one"),
            Error::BadTable => {
                f.write_str("fixup table bounds are reversed, misaligned or not whole rows")
            }
            Error::Unsorted { index } => {
                write!(f, "fixup table row {index} is not after the row before it")
            }
        }
    }
}
impl std::error::Error for Error {}

/// A fault being decided, as a region's callback sees it.
pub struct Fault {
    address: usize,
    pc: usize,
    /// The saved machine context (`ucontext_t`), to move the PC.
    context: *mut std::ffi::c_void,
}

impl Fault {
    /// The data address that faulted.
    pub fn address(&self) -> usize {
        self.address
    }
    /// The faulting instruction's address.
    pub fn pc(&self) -> usize {
        self.pc
    }
    /// Return from the handler to `pc` instead of retrying the
    /// instruction. Every other register is as the fault left it.
    pub fn resume_at(&mut self, pc: usize) {
        set_saved_pc(self.context, pc);
        self.pc = pc;
    }
}

#[cfg(unix)]
fn set_saved_pc(context: *mut std::ffi::c_void, pc: usize) {
    // SAFETY: `context` is the live saved context of the signal a Fault
    // was built for; only the handler builds one.
    unsafe { unix::set_pc(context.cast(), pc) }
}
#[cfg(not(unix))]
fn set_saved_pc(_: *mut std::ffi::c_void, _: usize) {}

/// Runs in the signal handler for a fault in its region: `true` when it
/// handled the fault (the thread resumes at [`Fault::pc`]), `false` to let
/// the next region or the previous handler have it. `data` is what was
/// registered with it.
pub type Callback = fn(&mut Fault, data: usize) -> bool;

/// The callback for a flat RAM window: resume a faulting access at its
/// landing label from the registered fixup tables, or decline.
pub fn fixup(fault: &mut Fault, _: usize) -> bool {
    match lookup(fault.pc()) {
        Some(landing) => {
            fault.resume_at(landing);
            true
        }
        None => false,
    }
}

struct RegionData {
    start: usize,
    /// Last address in the region (inclusive), so a range may end at the
    /// top of the address space.
    last: usize,
    callback: Callback,
    data: usize,
}

static REGIONS: [AtomicPtr<RegionData>; MAX_REGIONS] =
    [const { AtomicPtr::new(ptr::null_mut()) }; MAX_REGIONS];
/// Handlers (and lookups) reading the slot arrays right now.
static READERS: AtomicUsize = AtomicUsize::new(0);

/// Registration and removal are rare; serialise them so an overlap test
/// and a slot claim are one step and no entry is freed under them.
fn register_lock() -> std::sync::MutexGuard<'static, ()> {
    static REGISTER: std::sync::Mutex<()> = std::sync::Mutex::new(());
    REGISTER.lock().unwrap_or_else(|e| e.into_inner())
}

/// A registered region; dropping it unregisters it.
#[derive(Debug)]
pub struct Region {
    slot: usize,
}

/// Send faults whose data address is in `[start, start + len)` to
/// `callback`, installing the process handler if it is not yet.
pub fn register_region(
    start: usize,
    len: usize,
    callback: Callback,
    data: usize,
) -> Result<Region, Error> {
    let last = len
        .checked_sub(1)
        .and_then(|n| start.checked_add(n))
        .ok_or(Error::BadRange)?;
    install()?;
    let entry = Box::into_raw(Box::new(RegionData {
        start,
        last,
        callback,
        data,
    }));
    let _guard = register_lock();
    let mut free = None;
    for (i, slot) in REGIONS.iter().enumerate() {
        let p = slot.load(SeqCst);
        if p.is_null() {
            free.get_or_insert(i);
            continue;
        }
        // SAFETY: published entries are freed only by `Region::drop`, which
        // takes REGISTER first.
        let r = unsafe { &*p };
        if start <= r.last && r.start <= last {
            // SAFETY: never published.
            drop(unsafe { Box::from_raw(entry) });
            return Err(Error::Overlap);
        }
    }
    let Some(slot) = free else {
        // SAFETY: never published.
        drop(unsafe { Box::from_raw(entry) });
        return Err(Error::Full);
    };
    REGIONS[slot].store(entry, SeqCst);
    Ok(Region { slot })
}

impl Drop for Region {
    fn drop(&mut self) {
        let _guard = register_lock();
        let p = REGIONS[self.slot].swap(ptr::null_mut(), SeqCst);
        quiesce();
        if !p.is_null() {
            // SAFETY: unpublished above and no reader remains.
            drop(unsafe { Box::from_raw(p) });
        }
    }
}

/// Wait until no handler or lookup that might have seen an unpublished
/// entry is still running.
fn quiesce() {
    while READERS.load(SeqCst) != 0 {
        std::thread::yield_now();
    }
}

/// Offer a fault to the region holding `address`. Called by the handler.
#[cfg_attr(not(unix), allow(dead_code))]
fn dispatch(fault: &mut Fault) -> bool {
    READERS.fetch_add(1, SeqCst);
    let address = fault.address;
    let handled = REGIONS.iter().any(|slot| {
        let p = slot.load(SeqCst);
        if p.is_null() {
            return false;
        }
        // SAFETY: published entries outlive every reader (READERS above).
        let r = unsafe { &*p };
        r.start <= address && address <= r.last && (r.callback)(fault, r.data)
    });
    READERS.fetch_sub(1, SeqCst);
    handled
}

#[cfg(unix)]
fn install() -> Result<(), Error> {
    unix::install()
}
#[cfg(not(unix))]
fn install() -> Result<(), Error> {
    Err(Error::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decline(_: &mut Fault, _: usize) -> bool {
        false
    }

    #[test]
    fn regions_reject_empty_wrapping_and_overlapping_ranges() {
        if cfg!(not(unix)) {
            return;
        }
        assert_eq!(
            register_region(0x1000, 0, decline, 0).unwrap_err(),
            Error::BadRange
        );
        assert_eq!(
            register_region(usize::MAX, 2, decline, 0).unwrap_err(),
            Error::BadRange
        );
        // Synthetic ranges nothing maps: only the bookkeeping is tested.
        let base = 0x7f00_0000_0000usize;
        let a = register_region(base, 0x1000, decline, 0).unwrap();
        assert_eq!(
            register_region(base + 0xfff, 1, decline, 0).unwrap_err(),
            Error::Overlap
        );
        let b = register_region(base + 0x1000, 0x1000, decline, 0).unwrap();
        drop(a);
        let c = register_region(base, 0x1000, decline, 0).unwrap();
        drop((b, c));
    }
}
