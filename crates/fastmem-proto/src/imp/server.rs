//! The write-watch exception server.
//!
//! One dedicated handler thread receives `EXC_BAD_ACCESS` messages on a
//! receive right. Threads opt in with [`WatchServer::attach_current_thread`],
//! which installs that right as their thread-level exception port. See the
//! docs/perf/fastmem-proto.md for why thread-level ports rather than task-wide filtering.
//!
//! The handler path (everything reachable from `serve`) follows three rules:
//! no locks the stopped thread could hold (only atomics, Mach traps and the
//! `mprotect` syscall), no heap allocation (the receive buffer is on the
//! handler's stack, the tables are fixed arrays allocated at start), and no
//! unwinding (no indexing, no overflowing arithmetic, no `unwrap`).

use core::ffi::c_void;
use core::marker::PhantomData;
use core::mem::{MaybeUninit, size_of};
use core::ptr;
use core::sync::atomic::{AtomicI32, AtomicPtr, AtomicU32, AtomicU64, AtomicUsize, Ordering::*};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use super::sys::*;
use crate::Error;

/// Registered regions per server.
pub const MAX_REGIONS: usize = 16;
/// Attached threads per server.
pub const MAX_THREADS: usize = 64;

/// Private message id that tells the handler thread to exit.
const STOP_ID: i32 = 0x666d_0001;

const FREE: u32 = 0;
const LIVE: u32 = 2;

/// ESR_EL1 exception class for a data abort from a lower exception level.
const EC_DATA_ABORT_LOWER: u32 = 0x24;
const ESR_WNR: u32 = 1 << 6;
const ESR_CM: u32 = 1 << 8;

#[derive(Default)]
struct RegionSlot {
    state: AtomicU32,
    base: AtomicUsize,
    len: AtomicUsize,
    dirty: AtomicPtr<AtomicU64>,
    words: AtomicUsize,
}

#[derive(Default)]
struct ThreadSlot {
    state: AtomicU32,
    thread: AtomicU32,
    old_port: AtomicU32,
    old_behavior: AtomicI32,
}

#[derive(Default)]
struct Counters {
    handled: AtomicU64,
    forwarded: AtomicU64,
    forwarded_to_previous: AtomicU64,
    protect_failures: AtomicU64,
    unexpected_messages: AtomicU64,
}

/// Fault counters, read with [`WatchServer::stats`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Write faults in a registered region: marked dirty, opened, retried.
    pub handled: u64,
    /// Faults that were not ours (any reason), replied to or relayed.
    pub forwarded: u64,
    /// The subset of `forwarded` relayed to a saved previous handler port.
    pub forwarded_to_previous: u64,
    /// `mprotect` failed while opening a page; the fault was forwarded.
    pub protect_failures: u64,
    /// Messages on the port that were not exception requests.
    pub unexpected_messages: u64,
}

/// State shared with the handler thread. Fixed size; never reallocated.
struct Shared {
    page_shift: u32,
    regions: [RegionSlot; MAX_REGIONS],
    threads: [ThreadSlot; MAX_THREADS],
    /// Handler activity counter; teardown of a slot waits for it to drain.
    in_flight: AtomicU32,
    /// Saved task-level handler when attached task-wide (testing aid).
    task_fallback_port: AtomicU32,
    task_fallback_behavior: AtomicI32,
    /// Receive right the handler uses for relayed RPC replies.
    reply_port: mach_port_t,
    port: mach_port_t,
    counters: Counters,
}

impl Shared {
    fn quiesce(&self) {
        while self.in_flight.load(SeqCst) != 0 {
            core::hint::spin_loop();
        }
    }
}

/// Identifies a registered region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionId(usize);

/// A Mach exception server that turns first writes to protected pages of
/// registered regions into dirty bits.
pub struct WatchServer {
    shared: Arc<Shared>,
    handler: Option<JoinHandle<()>>,
    /// Owner-side bookkeeping; the handler never touches it.
    bitmaps: Mutex<[Option<Box<[AtomicU64]>>; MAX_REGIONS]>,
}

impl WatchServer {
    /// Allocates the exception port and starts the handler thread.
    pub fn start() -> Result<Self, Error> {
        let page = page_size();
        if !page.is_power_of_two() {
            return Err(Error::BadRange);
        }
        let task = task_self();
        let mut port = MACH_PORT_NULL;
        let mut reply_port = MACH_PORT_NULL;
        // SAFETY: plain Mach port calls on our own task.
        unsafe {
            kern(
                "mach_port_allocate",
                mach_port_allocate(task, MACH_PORT_RIGHT_RECEIVE, &mut port),
            )?;
            if let Err(e) = kern(
                "mach_port_insert_right",
                mach_port_insert_right(task, port, port, MACH_MSG_TYPE_MAKE_SEND),
            ) {
                mach_port_mod_refs(task, port, MACH_PORT_RIGHT_RECEIVE, -1);
                return Err(e);
            }
            if let Err(e) = kern(
                "mach_port_allocate",
                mach_port_allocate(task, MACH_PORT_RIGHT_RECEIVE, &mut reply_port),
            ) {
                destroy_port(port);
                return Err(e);
            }
        }
        let shared = Arc::new(Shared {
            page_shift: page.trailing_zeros(),
            regions: Default::default(),
            threads: core::array::from_fn(|_| ThreadSlot::default()),
            in_flight: AtomicU32::new(0),
            task_fallback_port: AtomicU32::new(MACH_PORT_NULL),
            task_fallback_behavior: AtomicI32::new(0),
            reply_port,
            port,
            counters: Counters::default(),
        });
        let for_handler = Arc::clone(&shared);
        let handler = std::thread::Builder::new()
            .name("fastmem-exc".into())
            .spawn(move || serve(&for_handler))
            .map_err(|_| Error::Kern("spawn handler thread", KERN_FAILURE))?;
        Ok(Self {
            shared,
            handler: Some(handler),
            bitmaps: Mutex::new(Default::default()),
        })
    }

    pub fn page_size(&self) -> usize {
        1 << self.shared.page_shift
    }

    /// The exception port's name in this task.
    pub fn port(&self) -> mach_port_t {
        self.shared.port
    }

    pub fn stats(&self) -> Stats {
        let c = &self.shared.counters;
        Stats {
            handled: c.handled.load(Relaxed),
            forwarded: c.forwarded.load(Relaxed),
            forwarded_to_previous: c.forwarded_to_previous.load(Relaxed),
            protect_failures: c.protect_failures.load(Relaxed),
            unexpected_messages: c.unexpected_messages.load(Relaxed),
        }
    }

    /// Registers `[base, base + len)` for write watch. Both must be page
    /// aligned. The region starts unarmed (writable, nothing dirty). The
    /// mapping must outlive the registration: unregister before unmapping.
    pub fn register(&self, base: *mut u8, len: usize) -> Result<RegionId, Error> {
        let page = self.page_size();
        let base = base as usize;
        if len == 0
            || !base.is_multiple_of(page)
            || !len.is_multiple_of(page)
            || base.checked_add(len).is_none()
        {
            return Err(Error::BadRange);
        }
        let mut bitmaps = self.bitmaps.lock().unwrap_or_else(|e| e.into_inner());
        for slot in &self.shared.regions {
            if slot.state.load(SeqCst) != LIVE {
                continue;
            }
            let (b, l) = (slot.base.load(Relaxed), slot.len.load(Relaxed));
            if base < b + l && b < base + len {
                return Err(Error::BadRange);
            }
        }
        let Some(index) = bitmaps.iter().position(Option::is_none) else {
            return Err(Error::Full);
        };
        let pages = len >> self.shared.page_shift;
        let words = pages.div_ceil(64);
        let bits: Box<[AtomicU64]> = (0..words).map(|_| AtomicU64::new(0)).collect();
        let slot = &self.shared.regions[index];
        slot.base.store(base, Relaxed);
        slot.len.store(len, Relaxed);
        slot.dirty.store(bits.as_ptr().cast_mut(), Relaxed);
        slot.words.store(words, Relaxed);
        slot.state.store(LIVE, SeqCst);
        bitmaps[index] = Some(bits);
        Ok(RegionId(index))
    }

    /// Unregisters a region and leaves its pages writable. The mapping must
    /// still be live. Dropping the server instead leaves pages as they are.
    pub fn unregister(&self, id: RegionId) -> Result<(), Error> {
        let mut bitmaps = self.bitmaps.lock().unwrap_or_else(|e| e.into_inner());
        if bitmaps.get(id.0).is_none_or(Option::is_none) {
            return Err(Error::BadRegion);
        }
        let slot = &self.shared.regions[id.0];
        slot.state.store(FREE, SeqCst);
        self.shared.quiesce();
        let (base, len) = (slot.base.load(Relaxed), slot.len.load(Relaxed));
        bitmaps[id.0] = None;
        protect(base, len, PROT_READ | PROT_WRITE)
    }

    fn region(&self, id: RegionId) -> Result<(&RegionSlot, usize, usize), Error> {
        let slot = self.shared.regions.get(id.0).ok_or(Error::BadRegion)?;
        if slot.state.load(SeqCst) != LIVE {
            return Err(Error::BadRegion);
        }
        Ok((slot, slot.base.load(Relaxed), slot.len.load(Relaxed)))
    }

    fn bits<'a>(&self, slot: &'a RegionSlot) -> &'a [AtomicU64] {
        // SAFETY: the bitmap lives in `self.bitmaps` while the slot is LIVE,
        // and the caller holds `&self`, which unregister also needs.
        unsafe { core::slice::from_raw_parts(slot.dirty.load(Relaxed), slot.words.load(Relaxed)) }
    }

    /// Arms the whole region: write-protects every page and clears its dirty
    /// bits. Writers must be quiesced (exclusive memory ownership).
    pub fn arm(&self, id: RegionId) -> Result<(), Error> {
        let (slot, base, len) = self.region(id)?;
        protect(base, len, PROT_READ)?;
        for w in self.bits(slot) {
            w.store(0, Release);
        }
        Ok(())
    }

    /// Takes the dirty set, calling `visit` with each dirty page index, and
    /// write-protects exactly those pages again (contiguous runs share one
    /// `mprotect`). Writers must be quiesced. Returns the dirty page count.
    pub fn rearm_dirty(&self, id: RegionId, mut visit: impl FnMut(usize)) -> Result<usize, Error> {
        let (slot, base, _) = self.region(id)?;
        let shift = self.shared.page_shift;
        let mut run: Option<(usize, usize)> = None;
        let mut count = 0;
        for (wi, w) in self.bits(slot).iter().enumerate() {
            let mut bits = w.swap(0, AcqRel);
            while bits != 0 {
                let page = wi * 64 + bits.trailing_zeros() as usize;
                bits &= bits - 1;
                visit(page);
                count += 1;
                run = match run {
                    Some((start, end)) if end == page => Some((start, page + 1)),
                    Some((start, end)) => {
                        protect(base + (start << shift), (end - start) << shift, PROT_READ)?;
                        Some((page, page + 1))
                    }
                    None => Some((page, page + 1)),
                };
            }
        }
        if let Some((start, end)) = run {
            protect(base + (start << shift), (end - start) << shift, PROT_READ)?;
        }
        Ok(count)
    }

    /// [`rearm_dirty`](Self::rearm_dirty) reported in guest-page units:
    /// `visit` gets every `guest_page`-sized subpage index of each dirty host
    /// page. Protection is per host page, so after the first fault the other
    /// subpages are written without faulting; reporting all of them is the
    /// conservative contract (false positives, never false negatives).
    /// `guest_page` must be a power of two no larger than the host page.
    pub fn rearm_dirty_subpages(
        &self,
        id: RegionId,
        guest_page: usize,
        mut visit: impl FnMut(usize),
    ) -> Result<usize, Error> {
        let host = self.page_size();
        if !guest_page.is_power_of_two() || guest_page > host {
            return Err(Error::BadRange);
        }
        let per = host / guest_page;
        self.rearm_dirty(id, |page| {
            for sub in 0..per {
                visit(page * per + sub);
            }
        })
    }

    /// Whether `page` (index within the region) is marked dirty.
    pub fn is_dirty(&self, id: RegionId, page: usize) -> Result<bool, Error> {
        let (slot, _, _) = self.region(id)?;
        let w = self.bits(slot).get(page / 64).ok_or(Error::BadRange)?;
        Ok(w.load(Acquire) & (1 << (page % 64)) != 0)
    }

    /// The dirty page indices, without clearing them.
    pub fn dirty_pages(&self, id: RegionId) -> Result<Vec<usize>, Error> {
        let (slot, _, len) = self.region(id)?;
        let pages = len >> self.shared.page_shift;
        let bits = self.bits(slot);
        Ok((0..pages)
            .filter(|p| bits[p / 64].load(Acquire) & (1 << (p % 64)) != 0)
            .collect())
    }

    /// Installs this server as the calling thread's `EXC_BAD_ACCESS` port.
    /// The previous thread-level handler is saved; faults that are not ours
    /// are relayed to it, or, when there was none, declined so the kernel
    /// continues to the task and host ports and finally the BSD signal.
    pub fn attach_current_thread(&self) -> Result<ThreadAttachment<'_>, Error> {
        let thread = thread_self();
        let shared = &self.shared;
        if shared
            .threads
            .iter()
            .any(|t| t.state.load(SeqCst) == LIVE && t.thread.load(Relaxed) == thread)
        {
            return Err(Error::AlreadyAttached);
        }
        // Claim a slot (state 1) before installing; it goes LIVE once the
        // saved handler is recorded. The attaching thread cannot fault into
        // a region in between, since it is running this function.
        let index = shared
            .threads
            .iter()
            .position(|t| t.state.compare_exchange(FREE, 1, SeqCst, Relaxed).is_ok())
            .ok_or(Error::Full)?;
        let mut saved = SavedPorts::default();
        // SAFETY: arrays are EXC_TYPES_COUNT long as the call requires.
        let kr = unsafe {
            thread_swap_exception_ports(
                thread,
                EXC_MASK_BAD_ACCESS,
                shared.port,
                EXCEPTION_DEFAULT | MACH_EXCEPTION_CODES,
                THREAD_STATE_NONE,
                saved.masks.as_mut_ptr(),
                &mut saved.count,
                saved.ports.as_mut_ptr(),
                saved.behaviors.as_mut_ptr(),
                saved.flavors.as_mut_ptr(),
            )
        };
        let slot = &shared.threads[index];
        if kr != KERN_SUCCESS {
            slot.state.store(FREE, SeqCst);
            return Err(Error::Kern("thread_swap_exception_ports", kr));
        }
        let (old_port, old_behavior) = saved.bad_access();
        slot.thread.store(thread, Relaxed);
        slot.old_port.store(old_port, Relaxed);
        slot.old_behavior.store(old_behavior, Relaxed);
        slot.state.store(LIVE, SeqCst);
        Ok(ThreadAttachment {
            server: self,
            index,
            thread,
            saved,
            _not_send: PhantomData,
        })
    }

    /// Installs this server as the task-wide `EXC_BAD_ACCESS` port. This is
    /// the alternative docs/perf/fastmem-proto.md argues against; it exists so tests can
    /// stand up a "previously installed" task-level handler to chain to.
    pub fn attach_task(&self) -> Result<TaskAttachment<'_>, Error> {
        let shared = &self.shared;
        let mut saved = SavedPorts::default();
        // SAFETY: arrays are EXC_TYPES_COUNT long as the call requires.
        let kr = unsafe {
            task_swap_exception_ports(
                task_self(),
                EXC_MASK_BAD_ACCESS,
                shared.port,
                EXCEPTION_DEFAULT | MACH_EXCEPTION_CODES,
                THREAD_STATE_NONE,
                saved.masks.as_mut_ptr(),
                &mut saved.count,
                saved.ports.as_mut_ptr(),
                saved.behaviors.as_mut_ptr(),
                saved.flavors.as_mut_ptr(),
            )
        };
        if kr != KERN_SUCCESS {
            return Err(Error::Kern("task_swap_exception_ports", kr));
        }
        let (old_port, old_behavior) = saved.bad_access();
        shared.task_fallback_behavior.store(old_behavior, Relaxed);
        shared.task_fallback_port.store(old_port, SeqCst);
        Ok(TaskAttachment {
            server: self,
            saved,
        })
    }
}

impl Drop for WatchServer {
    fn drop(&mut self) {
        let shared = &self.shared;
        // Attachments borrow the server, so none is live here.
        let mut stop = mach_msg_header_t {
            bits: mach_msgh_bits(MACH_MSG_TYPE_COPY_SEND, 0),
            size: size_of::<mach_msg_header_t>() as u32,
            remote_port: shared.port,
            id: STOP_ID,
            ..Default::default()
        };
        // SAFETY: a header-only message to our own port.
        let kr = unsafe {
            mach_msg(
                &mut stop,
                MACH_SEND_MSG,
                stop.size,
                0,
                MACH_PORT_NULL,
                0,
                MACH_PORT_NULL,
            )
        };
        if kr != MACH_MSG_SUCCESS {
            // Destroying the receive right also ends the handler's loop.
            // SAFETY: we own both rights under this name.
            unsafe { destroy_port(shared.port) };
        }
        // Join before the bitmaps (and the reply port) can go away.
        if let Some(h) = self.handler.take() {
            let _ = h.join();
        }
        // Regions are forgotten without touching their pages: the caller
        // may already have unmapped them, and the address range may since
        // belong to someone else. Armed pages stay read-only.
        // SAFETY: the handler has exited; nothing else uses these names.
        unsafe {
            if kr == MACH_MSG_SUCCESS {
                destroy_port(shared.port);
            }
            mach_port_mod_refs(task_self(), shared.reply_port, MACH_PORT_RIGHT_RECEIVE, -1);
        }
    }
}

#[derive(Default)]
struct SavedPorts {
    count: u32,
    masks: [exception_mask_t; EXC_TYPES_COUNT],
    ports: [mach_port_t; EXC_TYPES_COUNT],
    behaviors: [exception_behavior_t; EXC_TYPES_COUNT],
    flavors: [thread_state_flavor_t; EXC_TYPES_COUNT],
}

impl SavedPorts {
    fn entries(&self) -> impl Iterator<Item = usize> {
        0..(self.count as usize).min(EXC_TYPES_COUNT)
    }

    fn bad_access(&self) -> (mach_port_t, exception_behavior_t) {
        self.entries()
            .find(|&i| self.masks[i] & EXC_MASK_BAD_ACCESS != 0)
            .map_or((MACH_PORT_NULL, 0), |i| (self.ports[i], self.behaviors[i]))
    }

    /// Drops the send rights the swap gave us.
    fn release(&self) {
        for i in self.entries() {
            if valid_port(self.ports[i]) {
                // SAFETY: we own one reference from the swap.
                unsafe { mach_port_deallocate(task_self(), self.ports[i]) };
            }
        }
    }
}

/// Keeps the calling thread attached; dropping it restores the saved
/// thread-level handler. Drop attachments in reverse order of attaching.
pub struct ThreadAttachment<'a> {
    server: &'a WatchServer,
    index: usize,
    thread: mach_port_t,
    saved: SavedPorts,
    _not_send: PhantomData<*const ()>,
}

impl Drop for ThreadAttachment<'_> {
    fn drop(&mut self) {
        let s = &self.saved;
        let mut restored = false;
        for i in s.entries() {
            // SAFETY: restores exactly what thread_swap_exception_ports gave.
            unsafe {
                thread_set_exception_ports(
                    self.thread,
                    s.masks[i],
                    s.ports[i],
                    s.behaviors[i],
                    s.flavors[i],
                );
            }
            restored |= s.masks[i] & EXC_MASK_BAD_ACCESS != 0;
        }
        if !restored {
            // SAFETY: as above; no previous handler means a null port.
            unsafe {
                thread_set_exception_ports(
                    self.thread,
                    EXC_MASK_BAD_ACCESS,
                    MACH_PORT_NULL,
                    EXCEPTION_DEFAULT,
                    THREAD_STATE_NONE,
                );
            }
        }
        let shared = &self.server.shared;
        shared.threads[self.index].state.store(FREE, SeqCst);
        shared.quiesce();
        s.release();
    }
}

/// Keeps the server installed task-wide; dropping it restores the task's
/// previous handler.
pub struct TaskAttachment<'a> {
    server: &'a WatchServer,
    saved: SavedPorts,
}

impl Drop for TaskAttachment<'_> {
    fn drop(&mut self) {
        let s = &self.saved;
        for i in s.entries() {
            // SAFETY: restores what task_swap_exception_ports gave.
            unsafe {
                task_set_exception_ports(
                    task_self(),
                    s.masks[i],
                    s.ports[i],
                    s.behaviors[i],
                    s.flavors[i],
                );
            }
        }
        let shared = &self.server.shared;
        shared.task_fallback_port.store(MACH_PORT_NULL, SeqCst);
        shared.quiesce();
        s.release();
    }
}

fn valid_port(p: mach_port_t) -> bool {
    p != MACH_PORT_NULL && p != MACH_PORT_DEAD
}

fn kern(what: &'static str, kr: kern_return_t) -> Result<(), Error> {
    if kr == KERN_SUCCESS {
        Ok(())
    } else {
        Err(Error::Kern(what, kr))
    }
}

fn protect(base: usize, len: usize, prot: core::ffi::c_int) -> Result<(), Error> {
    // SAFETY: callers pass page-aligned ranges of registered mappings.
    if unsafe { mprotect(base as *mut c_void, len, prot) } == 0 {
        Ok(())
    } else {
        Err(Error::Errno(
            "mprotect",
            std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
        ))
    }
}

/// Drops the send right and the receive right a server port holds.
unsafe fn destroy_port(port: mach_port_t) {
    // SAFETY: the caller owns both rights under this name.
    unsafe {
        mach_port_deallocate(task_self(), port);
        mach_port_mod_refs(task_self(), port, MACH_PORT_RIGHT_RECEIVE, -1);
    }
}

// ---------------------------------------------------------------------------
// Handler path. Nothing below may lock, allocate or unwind.
// ---------------------------------------------------------------------------

/// Receive buffer: the 84-byte request plus the largest trailer, rounded up.
#[repr(C, align(8))]
struct RecvBuffer([u8; 512]);

fn serve(shared: &Shared) {
    // A faulting thread is stopped until we reply, so the handler runs at the
    // highest QoS a normal thread can ask for. Failure only costs latency.
    // SAFETY: affects the calling thread only.
    unsafe { pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0) };
    let mut buf = MaybeUninit::<RecvBuffer>::uninit();
    let hdr = buf.as_mut_ptr().cast::<mach_msg_header_t>();
    loop {
        // SAFETY: the buffer is ours and 512 bytes long.
        let kr = unsafe { mach_msg(hdr, MACH_RCV_MSG, 0, 512, shared.port, 0, MACH_PORT_NULL) };
        if kr != MACH_MSG_SUCCESS {
            // MACH_RCV_TOO_LARGE drops the message; anything else means the
            // port is gone. Either way there is nothing to reply to.
            if kr == 0x1000_4004 {
                shared.counters.unexpected_messages.fetch_add(1, Relaxed);
                continue;
            }
            return;
        }
        // SAFETY: mach_msg wrote at least a header.
        let head = unsafe { ptr::read(hdr) };
        if head.id == STOP_ID {
            return;
        }
        let complex = head.bits & MACH_MSGH_BITS_COMPLEX != 0;
        if head.id != MACH_EXCEPTION_RAISE_ID
            || !complex
            || (head.size as usize) < size_of::<ExceptionRaise64>()
        {
            shared.counters.unexpected_messages.fetch_add(1, Relaxed);
            // SAFETY: releases whatever rights the message carried.
            unsafe { mach_msg_destroy(hdr) };
            continue;
        }
        // SAFETY: the size check above covers the whole request.
        let req = unsafe { ptr::read_unaligned(hdr.cast::<ExceptionRaise64>()) };

        shared.in_flight.fetch_add(1, SeqCst);
        let ret = handle(shared, &req);
        shared.in_flight.fetch_sub(1, SeqCst);

        let mut reply = ExceptionReply {
            head: mach_msg_header_t {
                bits: mach_msgh_bits(head.bits & 0x1f, 0),
                size: size_of::<ExceptionReply>() as u32,
                remote_port: head.remote_port,
                local_port: MACH_PORT_NULL,
                voucher_port: MACH_PORT_NULL,
                id: head.id.wrapping_add(MIG_REPLY_DELTA),
            },
            ndr: NDR_RECORD,
            ret_code: ret,
        };
        // SAFETY: replies on the send-once right the kernel gave us; the
        // reply carries no rights of ours. The thread and task names came
        // in as send rights we now own and must drop.
        unsafe {
            mach_msg(
                ptr::addr_of_mut!(reply.head),
                MACH_SEND_MSG,
                reply.head.size,
                0,
                MACH_PORT_NULL,
                0,
                MACH_PORT_NULL,
            );
            let thread = req.thread;
            let task = req.task;
            mach_port_deallocate(task_self(), thread.name);
            mach_port_deallocate(task_self(), task.name);
        }
    }
}

/// Decides one exception: `KERN_SUCCESS` resumes the thread at the same PC,
/// which retries the faulting instruction.
fn handle(shared: &Shared, req: &ExceptionRaise64) -> kern_return_t {
    let thread = req.thread.name;
    let codes = req.code;
    if req.exception != EXC_BAD_ACCESS
        || req.code_count < 2
        || codes[0] != KERN_PROTECTION_FAILURE as i64
    {
        return forward(shared, req);
    }
    let mut state = arm_exception_state64::default();
    let mut count = ARM_EXCEPTION_STATE64_COUNT;
    // SAFETY: state is ARM_EXCEPTION_STATE64_COUNT words.
    let kr = unsafe {
        thread_get_state(
            thread,
            ARM_EXCEPTION_STATE64,
            ptr::addr_of_mut!(state).cast(),
            &mut count,
        )
    };
    let ec = state.esr >> 26;
    let is_write = state.esr & ESR_WNR != 0 && state.esr & ESR_CM == 0;
    if kr != KERN_SUCCESS || ec != EC_DATA_ABORT_LOWER || !is_write {
        return forward(shared, req);
    }
    match mark_and_open(shared, codes[1] as usize) {
        Some(true) => {
            shared.counters.handled.fetch_add(1, Relaxed);
            KERN_SUCCESS
        }
        Some(false) => {
            shared.counters.protect_failures.fetch_add(1, Relaxed);
            forward(shared, req)
        }
        None => forward(shared, req),
    }
}

/// Marks the page holding `addr` dirty, then makes it writable. The mark
/// comes first so a drain that finds the page writable also finds the bit.
fn mark_and_open(shared: &Shared, addr: usize) -> Option<bool> {
    for slot in &shared.regions {
        if slot.state.load(SeqCst) != LIVE {
            continue;
        }
        let base = slot.base.load(Relaxed);
        let offset = addr.wrapping_sub(base);
        if offset >= slot.len.load(Relaxed) {
            continue;
        }
        let page = offset >> shared.page_shift;
        let word = page / 64;
        if word >= slot.words.load(Relaxed) {
            return Some(false);
        }
        // SAFETY: word < words, and the bitmap outlives the LIVE state
        // because unregister waits for `in_flight` to drain.
        let bits = unsafe { &*slot.dirty.load(Relaxed).add(word) };
        bits.fetch_or(1 << (page % 64), Release);
        let page_base = base.wrapping_add(page << shared.page_shift);
        // SAFETY: a page of a registered mapping.
        let ok = unsafe {
            mprotect(
                page_base as *mut c_void,
                1 << shared.page_shift,
                PROT_READ | PROT_WRITE,
            )
        } == 0;
        return Some(ok);
    }
    None
}

/// Not our fault. Relay it to the thread's saved handler when it takes
/// default-behaviour messages; otherwise decline with `KERN_FAILURE`, which
/// makes the kernel try the next level (task, then host, then signal).
fn forward(shared: &Shared, req: &ExceptionRaise64) -> kern_return_t {
    shared.counters.forwarded.fetch_add(1, Relaxed);
    let thread = req.thread.name;
    let mut target = None;
    for slot in &shared.threads {
        if slot.state.load(SeqCst) == LIVE && slot.thread.load(Relaxed) == thread {
            target = Some((slot.old_port.load(Relaxed), slot.old_behavior.load(Relaxed)));
            break;
        }
    }
    if target.is_none() {
        let port = shared.task_fallback_port.load(SeqCst);
        target = Some((port, shared.task_fallback_behavior.load(Relaxed)));
    }
    let Some((port, behavior)) = target else {
        return KERN_FAILURE;
    };
    if !valid_port(port) || port == shared.port {
        return KERN_FAILURE;
    }
    if behavior & !MACH_EXCEPTION_CODES != EXCEPTION_DEFAULT {
        // State-carrying behaviours would need thread_get_state per flavor;
        // the prototype declines and lets the kernel fall through instead.
        return KERN_FAILURE;
    }
    shared.counters.forwarded_to_previous.fetch_add(1, Relaxed);
    relay(shared, req, port, behavior & MACH_EXCEPTION_CODES != 0)
}

/// Sends the exception to `port` as a MIG `exception_raise` (32-bit codes)
/// or `mach_exception_raise` (64-bit codes) RPC and returns its verdict.
fn relay(
    shared: &Shared,
    req: &ExceptionRaise64,
    port: mach_port_t,
    code64: bool,
) -> kern_return_t {
    #[repr(C, align(8))]
    union Msg {
        r64: ExceptionRaise64,
        r32: ExceptionRaise32,
        reply: ExceptionReply,
        raw: [u8; 256],
    }
    let head = mach_msg_header_t {
        bits: MACH_MSGH_BITS_COMPLEX
            | mach_msgh_bits(MACH_MSG_TYPE_COPY_SEND, MACH_MSG_TYPE_MAKE_SEND_ONCE),
        size: 0,
        remote_port: port,
        local_port: shared.reply_port,
        voucher_port: MACH_PORT_NULL,
        id: 0,
    };
    let thread = mach_msg_port_descriptor_t::new(req.thread.name, MACH_MSG_TYPE_COPY_SEND);
    let task = mach_msg_port_descriptor_t::new(req.task.name, MACH_MSG_TYPE_COPY_SEND);
    let codes = req.code;
    let (mut msg, request_id, send_size) = if code64 {
        let r = ExceptionRaise64 {
            head: mach_msg_header_t {
                size: size_of::<ExceptionRaise64>() as u32,
                id: MACH_EXCEPTION_RAISE_ID,
                ..head
            },
            descriptor_count: 2,
            thread,
            task,
            ndr: NDR_RECORD,
            exception: req.exception,
            code_count: 2,
            code: codes,
        };
        (
            Msg { r64: r },
            MACH_EXCEPTION_RAISE_ID,
            size_of::<ExceptionRaise64>(),
        )
    } else {
        let r = ExceptionRaise32 {
            head: mach_msg_header_t {
                size: size_of::<ExceptionRaise32>() as u32,
                id: EXCEPTION_RAISE_ID,
                ..head
            },
            descriptor_count: 2,
            thread,
            task,
            ndr: NDR_RECORD,
            exception: req.exception,
            code_count: 2,
            code: [codes[0] as i32, codes[1] as i32],
        };
        (
            Msg { r32: r },
            EXCEPTION_RAISE_ID,
            size_of::<ExceptionRaise32>(),
        )
    };
    // SAFETY: the union is 256 bytes and starts with a header.
    let kr = unsafe {
        let hdr = ptr::addr_of_mut!(msg).cast::<mach_msg_header_t>();
        mach_msg(
            hdr,
            MACH_SEND_MSG | MACH_RCV_MSG,
            send_size as u32,
            size_of::<Msg>() as u32,
            shared.reply_port,
            0,
            MACH_PORT_NULL,
        )
    };
    if kr != MACH_MSG_SUCCESS {
        return KERN_FAILURE;
    }
    // SAFETY: a successful receive wrote at least a header.
    let reply = unsafe { msg.reply };
    let reply_head = reply.head;
    if reply_head.id != request_id.wrapping_add(MIG_REPLY_DELTA)
        || (reply_head.size as usize) < size_of::<ExceptionReply>()
    {
        // E.g. MACH_NOTIFY_SEND_ONCE when the old handler died unanswered.
        // SAFETY: releases any rights the stray message carried.
        unsafe { mach_msg_destroy(ptr::addr_of_mut!(msg).cast()) };
        return KERN_FAILURE;
    }
    reply.ret_code
}
