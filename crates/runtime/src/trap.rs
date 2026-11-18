//! Access trap: which guest pages are read and written, and by whom, for
//! research observers (the `render-census` feature,
//! docs/graphics/pipelined-render.md). Off unless installed; with no trap
//! installed nothing changes on any path.
//!
//! Arming a page clears its read and/or write bit in the fast page table
//! (`h2fastmem.h` and [`Guest::read_bytes`]'s inline case), so the next
//! access of that kind, from generated C or from Rust, takes the checked
//! slow path. No host page protection and no signal handler: the fast table
//! already is a software MMU every inline access consults. The slow path
//! logs a [`TrapHit`] for an armed page, then:
//!
//! - a hit from guest code (the ABI helpers pass its `pc`) disarms that
//!   kind on that page, so each page costs at most one slow read and one
//!   slow write per arming;
//! - a hit from the host (HLE, Rust ports of guest code: `pc` 0) leaves the
//!   page armed and logs only the first such hit per kind, so a page the
//!   host writes first still reports the guest code that writes it later;
//! - a *sticky* page is never disarmed: its first guest and first host hit
//!   of each kind are logged as above, and every access adds to its
//!   64-byte line masks per guest thread ([`LineRec`]), for line-level
//!   detail without a log entry per access.
//!
//! Each hit carries the guest thread the kernel says is running
//! (`timeline::phase::running_guest_thread`) and, for host hits, the Rust
//! handler (kernel export or override) the runner is executing
//! ([`hle_enter`]). With the HLE census on ([`set_hle_census`]) the runner
//! also counts Rust-handler calls by target and thread ([`take_hle_calls`]).
//!
//! Accesses that never consult the fast table (raw [`Guest::physical_ram`]
//! readers) are not seen.
//!
//! [`Guest::read_bytes`]: crate::Guest::read_bytes
//! [`Guest::physical_ram`]: crate::Guest::physical_ram
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const PAGES: usize = 1 << 20;
const WORDS: usize = PAGES / 64;

/// One logged access to an armed page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrapHit {
    /// The guest virtual address and length of the access.
    pub addr: u32,
    pub len: u32,
    pub write: bool,
    /// The guest instruction, from the ABI helpers; 0 for the host.
    pub pc: u32,
    /// [`timeline::phase::Phase`] at the access (`as u8`), or `u8::MAX`
    /// while frame stats are off.
    pub phase: u8,
    /// The running guest thread, or [`NO_THREAD`] (between slices, or frame
    /// stats off).
    pub thread: u32,
    /// The Rust handler the runner is executing (its target address: a
    /// kernel token or an override's entry), or 0.
    pub hle: u32,
}

/// [`TrapHit::thread`] when no guest thread is known.
pub const NO_THREAD: u32 = u32::MAX;

/// A sticky page's 64-byte lines touched by one guest thread since the last
/// take (bit `i` is line `i`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LineRec {
    pub page: u32,
    pub thread: u32,
    pub read: u64,
    pub write: u64,
    /// Lines read before this thread wrote them in this log.
    pub read_first: u64,
    /// Accesses counted.
    pub accesses: u64,
}

/// The logged hits since the last [`take`](crate::Guest::take_trap_hits),
/// and how many the cap dropped.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TrapLog {
    pub hits: Vec<TrapHit>,
    pub dropped: u64,
    /// Sticky pages' lines, by page and thread, in no order.
    pub lines: Vec<LineRec>,
}

#[derive(Default)]
struct Log {
    hits: Vec<TrapHit>,
    dropped: u64,
    lines: HashMap<(u32, u32), LineRec>,
}

fn bits() -> Box<[AtomicU64]> {
    (0..WORDS).map(|_| AtomicU64::new(0)).collect()
}

#[inline]
fn test(b: &[AtomicU64], page: usize) -> bool {
    b[page / 64].load(Ordering::Relaxed) & (1 << (page % 64)) != 0
}
#[inline]
fn set(b: &[AtomicU64], page: usize) {
    b[page / 64].fetch_or(1 << (page % 64), Ordering::Relaxed);
}
#[inline]
fn clear(b: &[AtomicU64], page: usize) {
    b[page / 64].fetch_and(!(1 << (page % 64)), Ordering::Relaxed);
}
/// Sets bit `page`; true if it was clear.
#[inline]
fn claim(b: &[AtomicU64], page: usize) -> bool {
    let m = 1 << (page % 64);
    b[page / 64].fetch_or(m, Ordering::Relaxed) & m == 0
}

/// The trap's state inside one [`crate::Guest`]. The bit sets are atomic so
/// a host read through `&Guest` can log.
pub(crate) struct AccessTrap {
    /// Armed for reads / writes, by virtual page.
    read: Box<[AtomicU64]>,
    write: Box<[AtomicU64]>,
    sticky: Box<[AtomicU64]>,
    /// The host's hit of that kind is already logged for this arming.
    host_read: Box<[AtomicU64]>,
    host_write: Box<[AtomicU64]>,
    /// A sticky page's guest hit of that kind is already logged.
    guest_read: Box<[AtomicU64]>,
    guest_write: Box<[AtomicU64]>,
    log: Mutex<Log>,
    cap: usize,
}

impl AccessTrap {
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            read: bits(),
            write: bits(),
            sticky: bits(),
            host_read: bits(),
            host_write: bits(),
            guest_read: bits(),
            guest_write: bits(),
            log: Mutex::new(Log::default()),
            cap,
        }
    }
    pub(crate) fn arm(&self, page: usize, read: bool, write: bool) {
        if read {
            set(&self.read, page);
            clear(&self.host_read, page);
            clear(&self.guest_read, page);
        }
        if write {
            set(&self.write, page);
            clear(&self.host_write, page);
            clear(&self.guest_write, page);
        }
    }
    /// Disarms both kinds and stickiness; true if the page was armed.
    pub(crate) fn disarm(&self, page: usize) -> bool {
        let was = test(&self.read, page) || test(&self.write, page);
        clear(&self.read, page);
        clear(&self.write, page);
        clear(&self.sticky, page);
        was
    }
    pub(crate) fn set_sticky(&self, page: usize, on: bool) {
        if on {
            set(&self.sticky, page);
        } else {
            clear(&self.sticky, page);
        }
    }
    pub(crate) fn armed(&self, page: usize, write: bool) -> bool {
        test(if write { &self.write } else { &self.read }, page)
    }
    /// Every page armed for either kind, ascending.
    pub(crate) fn armed_pages(&self) -> Vec<usize> {
        let mut out = Vec::new();
        for w in 0..WORDS {
            let mut m =
                self.read[w].load(Ordering::Relaxed) | self.write[w].load(Ordering::Relaxed);
            while m != 0 {
                out.push(w * 64 + m.trailing_zeros() as usize);
                m &= m - 1;
            }
        }
        out
    }

    /// An access to `[addr, addr + len)` that the slow path allowed. Logs
    /// what the rules above say; returns the pages whose `write`/read kind
    /// the caller must disarm (guest hits on non-sticky pages). `len` is
    /// non-zero.
    pub(crate) fn note(&self, pc: u32, addr: u32, len: usize, write: bool) -> PagesToOpen {
        let first = (addr >> 12) as usize;
        let last = ((addr as u64 + len as u64 - 1) >> 12) as usize;
        let mut open = PagesToOpen::default();
        let mut logged = false;
        let thread = timeline::phase::running_guest_thread().unwrap_or(NO_THREAD);
        for page in first..=last.min(PAGES - 1) {
            if !self.armed(page, write) {
                continue;
            }
            let sticky = test(&self.sticky, page);
            if sticky {
                self.add_lines(page as u32, thread, addr, len, write);
            }
            let seen = match (pc == 0, write) {
                (true, true) => Some(&self.host_write),
                (true, false) => Some(&self.host_read),
                (false, true) if sticky => Some(&self.guest_write),
                (false, false) if sticky => Some(&self.guest_read),
                _ => None,
            };
            if seen.is_some_and(|seen| !claim(seen, page)) {
                continue;
            }
            if pc != 0 && !sticky {
                if write {
                    clear(&self.write, page);
                } else {
                    clear(&self.read, page);
                }
                open.push(page);
            }
            if !logged {
                logged = true;
                self.push(TrapHit {
                    addr,
                    len: len as u32,
                    write,
                    pc,
                    phase: timeline::phase::current().map_or(u8::MAX, |p| p as u8),
                    thread,
                    hle: if pc == 0 { HLE_NOW.with(Cell::get) } else { 0 },
                });
            }
        }
        open
    }
    fn add_lines(&self, page: u32, thread: u32, addr: u32, len: usize, write: bool) {
        let lo = (page as u64) << 12;
        let (a, b) = (
            (addr as u64).max(lo),
            (addr as u64 + len as u64).min(lo + 4096),
        );
        if a >= b {
            return;
        }
        let m = (u64::MAX >> (63 - (b - 1 - lo) / 64)) & (u64::MAX << ((a - lo) / 64));
        let mut log = self.log.lock().unwrap_or_else(|e| e.into_inner());
        let e = log.lines.entry((page, thread)).or_insert(LineRec {
            page,
            thread,
            ..LineRec::default()
        });
        e.accesses += 1;
        if write {
            e.write |= m;
        } else {
            e.read_first |= m & !e.write;
            e.read |= m;
        }
    }
    fn push(&self, hit: TrapHit) {
        let mut log = self.log.lock().unwrap_or_else(|e| e.into_inner());
        if log.hits.len() < self.cap {
            log.hits.push(hit);
        } else {
            log.dropped += 1;
        }
    }
    pub(crate) fn take(&self) -> TrapLog {
        let log = std::mem::take(&mut *self.log.lock().unwrap_or_else(|e| e.into_inner()));
        TrapLog {
            hits: log.hits,
            dropped: log.dropped,
            lines: log.lines.into_values().collect(),
        }
    }
}

static HLE_ON: AtomicBool = AtomicBool::new(false);
static HLE_CALLS: Mutex<Option<HashMap<(u32, u32), HleCall>>> = Mutex::new(None);
thread_local! {
    static HLE_NOW: Cell<u32> = const { Cell::new(0) };
}

/// Rust-handler calls the runner made to one target on one guest thread
/// since the last [`take_hle_calls`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct HleCall {
    /// The handler's target: a kernel token (`kernel::KERNEL_TOKEN_BASE +
    /// 4 * ordinal`) or an override's entry.
    pub target: u32,
    /// The guest thread, or [`NO_THREAD`].
    pub thread: u32,
    pub calls: u64,
    /// Calls that returned a STOP: the thread blocked, yielded or was
    /// preempted inside the call.
    pub stops: u64,
}

/// Turns the runner's Rust-handler census on or off (process-wide; off by
/// default, when the runner pays one relaxed load per Rust-handler call).
pub fn set_hle_census(on: bool) {
    HLE_ON.store(on, Ordering::Relaxed);
}

/// Whether [`set_hle_census`] is on.
#[inline]
pub fn hle_census() -> bool {
    HLE_ON.load(Ordering::Relaxed)
}

/// The runner is about to execute the Rust handler for `target`: counts the
/// call and makes it the current handler for host hits. Returns the one it
/// replaces, for [`hle_leave`].
pub fn hle_enter(target: u32) -> u32 {
    let thread = timeline::phase::running_guest_thread().unwrap_or(NO_THREAD);
    hle_count(target, thread, |c| c.calls += 1);
    HLE_NOW.with(|n| n.replace(target))
}

/// The handler [`hle_enter`] started returned (`stopped`: with a STOP).
/// `thread` is the guest thread it was entered on.
pub fn hle_leave(prev: u32, target: u32, thread: u32, stopped: bool) {
    HLE_NOW.with(|n| n.set(prev));
    if stopped {
        hle_count(target, thread, |c| c.stops += 1);
    }
}

fn hle_count(target: u32, thread: u32, f: impl FnOnce(&mut HleCall)) {
    let mut m = HLE_CALLS.lock().unwrap_or_else(|e| e.into_inner());
    let e = m
        .get_or_insert_with(HashMap::new)
        .entry((target, thread))
        .or_insert(HleCall {
            target,
            thread,
            ..HleCall::default()
        });
    f(e);
}

/// The Rust-handler calls counted since the last take, in no order.
pub fn take_hle_calls() -> Vec<HleCall> {
    HLE_CALLS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .map(|m| m.into_values().collect())
        .unwrap_or_default()
}

/// The pages a guest hit disarmed: at most two, as the ABI's widths cross
/// at most one page boundary. The host never disarms.
#[derive(Default)]
pub(crate) struct PagesToOpen(Vec<usize>);
impl PagesToOpen {
    fn push(&mut self, page: usize) {
        self.0.push(page);
    }
    pub(crate) fn pages(&self) -> &[usize] {
        &self.0
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use super::*;
    use crate::guest::{PAGE_SIZE, Permissions};
    use crate::{Dispatch, ExecCtx, Guest, Runtime};
    use std::sync::Arc;

    const RW: u32 = 0x0001_0000;
    const RO: u32 = 0x0003_0000;

    fn guest() -> Guest {
        let mut g = Guest::new();
        g.map(RW, 4 * PAGE_SIZE, 0x0010_0000, Permissions::RW)
            .unwrap();
        g.map(RO, PAGE_SIZE, 0x0012_0000, Permissions::READ)
            .unwrap();
        g.prepare_fast_pages();
        g.install_trap(1 << 16);
        g
    }

    /// The fast page table's permission bits for `va`'s page.
    fn fast(g: &mut Guest, va: u32) -> u32 {
        let (_, pages, _) = g.fast_parts();
        // SAFETY: the table has 1 << 20 entries and outlives this read.
        unsafe { (*pages.add((va >> 12) as usize)).permissions }
    }

    fn hits(g: &mut Guest) -> Vec<(u32, bool, u32)> {
        let log = g.take_trap_hits();
        assert_eq!(log.dropped, 0);
        log.hits.iter().map(|h| (h.addr, h.write, h.pc)).collect()
    }

    #[test]
    fn a_guest_hit_disarms_its_kind_on_its_page_only() {
        let mut g = guest();
        g.arm_all(true, true);
        assert_eq!(fast(&mut g, RW) & 3, 0, "armed: no inline read or write");
        g.write_bytes_from(0x1234, RW + 8, &[1, 2, 3, 4], 1)
            .unwrap();
        g.write_bytes_from(0x1238, RW + 12, &[5], 1).unwrap();
        assert_eq!(
            hits(&mut g),
            [(RW + 8, true, 0x1234)],
            "the first write only"
        );
        assert_eq!(
            fast(&mut g, RW) & 3,
            2,
            "writes inline again, reads still armed"
        );
        let mut b = [0; 4];
        g.read_bytes_from(0x2000, RW + 8, &mut b, 1).unwrap();
        g.read_bytes_from(0x2004, RW + 8, &mut b, 1).unwrap();
        assert_eq!(b, [1, 2, 3, 4]);
        assert_eq!(hits(&mut g), [(RW + 8, false, 0x2000)]);
        assert_eq!(fast(&mut g, RW) & 3, 3);
        assert_eq!(
            fast(&mut g, RW + PAGE_SIZE) & 3,
            0,
            "the next page is still armed"
        );
    }

    #[test]
    fn the_host_is_logged_once_and_never_disarms() {
        let mut g = guest();
        g.arm_all(false, true);
        g.write_bytes(RW, &[1], 1).unwrap();
        g.set_u32(RW + 4, 7).unwrap();
        assert_eq!(
            hits(&mut g),
            [(RW, true, 0)],
            "one host hit per page and kind"
        );
        assert_eq!(fast(&mut g, RW) & 2, 0, "still armed for the guest");
        g.write_bytes_from(0x4000, RW + 16, &[9], 1).unwrap();
        assert_eq!(hits(&mut g), [(RW + 16, true, 0x4000)]);
        // Reads were never armed.
        assert_eq!(g.u32(RW + 4).unwrap(), 7);
        assert!(hits(&mut g).is_empty());
        // Re-arming clears the host's mark.
        g.arm_all(false, true);
        g.set_u8(RW + 1, 1).unwrap();
        assert_eq!(hits(&mut g), [(RW + 1, true, 0)]);
    }

    #[test]
    fn a_span_across_pages_logs_once_and_opens_both() {
        let mut g = guest();
        g.arm_all(true, true);
        let at = RW + PAGE_SIZE - 2;
        g.write_bytes_from(0x10, at, &[1, 2, 3, 4], 1).unwrap();
        assert_eq!(hits(&mut g), [(at, true, 0x10)]);
        assert_eq!(fast(&mut g, RW) & 2, 2);
        assert_eq!(fast(&mut g, RW + PAGE_SIZE) & 2, 2);
    }

    #[test]
    fn faults_log_nothing_and_permissions_come_back_exactly() {
        let mut g = guest();
        g.arm_all(true, true);
        assert!(g.write_bytes_from(0x10, RO, &[1], 1).is_err());
        assert!(hits(&mut g).is_empty());
        let mut b = [0];
        g.read_bytes_from(0x20, RO, &mut b, 1).unwrap();
        assert_eq!(hits(&mut g), [(RO, false, 0x20)]);
        assert_eq!(fast(&mut g, RO) & 3, 1, "opened for reads, never writable");
        g.disarm_all();
        assert_eq!(fast(&mut g, RO) & 3, 1);
        for p in 0..4 {
            assert_eq!(fast(&mut g, RW + p * PAGE_SIZE) & 3, 3);
        }
        g.write_bytes_from(0x30, RW, &[1], 1).unwrap();
        assert!(hits(&mut g).is_empty(), "disarmed");
    }

    #[test]
    fn sticky_pages_count_every_access_in_lines_and_a_protect_keeps_the_arming() {
        let mut g = guest();
        g.arm_all(true, true);
        g.set_sticky_pages(&[RW >> 12]);
        for i in 0..3 {
            g.write_bytes_from(0x50 + i, RW + 64 * i, &[1], 1).unwrap();
            g.write_bytes(RW + 64 * i + 1, &[1], 1).unwrap();
        }
        let mut b = [0];
        // Line 1 was written first, line 5 not.
        g.read_bytes_from(0x60, RW + 64, &mut b, 1).unwrap();
        g.read_bytes_from(0x64, RW + 5 * 64, &mut b, 1).unwrap();
        let log = g.take_trap_hits();
        assert_eq!(
            log.hits
                .iter()
                .map(|h| (h.addr, h.write, h.pc))
                .collect::<Vec<_>>(),
            [(RW, true, 0x50), (RW + 1, true, 0), (RW + 64, false, 0x60)],
            "the first guest and first host hit of each kind, once per arming"
        );
        assert_eq!(
            log.lines,
            [LineRec {
                page: RW >> 12,
                thread: NO_THREAD,
                read: 0b10_0010,
                write: 0b111,
                read_first: 0b10_0000,
                accesses: 8,
            }]
        );
        g.arm_all(true, true);
        g.write_bytes_from(0x70, RW + 4, &[1], 1).unwrap();
        assert_eq!(
            hits(&mut g),
            [(RW + 4, true, 0x70)],
            "re-armed: logged again"
        );
        assert_eq!(fast(&mut g, RW) & 3, 0);
        // A protect rebuilds the fast entries; armed kinds stay cleared.
        g.protect(RW, Permissions::RW).unwrap();
        assert_eq!(fast(&mut g, RW + PAGE_SIZE) & 3, 0);
        g.remove_trap();
        assert!(!g.has_trap());
        assert_eq!(fast(&mut g, RW) & 3, 3);
    }

    #[test]
    fn the_abi_helpers_attribute_generated_code_accesses_to_their_pc() {
        let mut g = guest();
        g.arm_all(true, true);
        let rt = Runtime::new(g, Dispatch::default());
        let mut ctx = ExecCtx::new(Arc::clone(&rt), 0);
        let exec = (&mut ctx as *mut ExecCtx).cast();
        let v = [1u8, 2, 3, 4];
        let mut out = [0u8; 4];
        // SAFETY: the context is live and exclusive; buffers are disjoint.
        unsafe {
            assert_eq!(
                crate::ffi::h2abi_v1_write(exec, 0x0040_1000, RW, 4, 1, v.as_ptr()),
                0
            );
            assert_eq!(
                crate::ffi::h2abi_v1_read(exec, 0x0040_2000, RW, 4, 1, out.as_mut_ptr()),
                0
            );
        }
        assert_eq!(out, v);
        let mut m = rt.memory();
        assert_eq!(
            hits(&mut m),
            [(RW, true, 0x0040_1000), (RW, false, 0x0040_2000)]
        );
        assert_eq!(fast(&mut m, RW) & 3, 3);
    }

    #[test]
    fn host_hits_name_the_rust_handler_and_its_calls_are_counted() {
        let mut g = guest();
        g.arm_all(false, true);
        // A target no other test uses: the call table is process-wide.
        const T: u32 = 0xfff0_0aa8;
        let prev = hle_enter(T);
        g.set_u32(RW, 1).unwrap();
        hle_leave(prev, T, NO_THREAD, true);
        g.set_u32(RW + PAGE_SIZE, 1).unwrap();
        let log = g.take_trap_hits();
        assert_eq!(
            log.hits.iter().map(|h| (h.addr, h.hle)).collect::<Vec<_>>(),
            [(RW, T), (RW + PAGE_SIZE, 0)]
        );
        let calls: Vec<_> = take_hle_calls()
            .into_iter()
            .filter(|c| c.target == T)
            .collect();
        assert_eq!(
            calls,
            [HleCall {
                target: T,
                thread: NO_THREAD,
                calls: 1,
                stops: 1
            }]
        );
    }

    #[test]
    fn the_cap_drops_and_counts() {
        let mut g = Guest::new();
        g.map(RW, 4 * PAGE_SIZE, 0x0010_0000, Permissions::RW)
            .unwrap();
        g.install_trap(2);
        g.arm_all(true, true);
        for p in 0..4 {
            g.write_bytes_from(0x10, RW + p * PAGE_SIZE, &[1], 1)
                .unwrap();
        }
        let log = g.take_trap_hits();
        assert_eq!((log.hits.len(), log.dropped), (2, 2));
        assert_eq!(g.take_trap_hits(), TrapLog::default());
    }
}
