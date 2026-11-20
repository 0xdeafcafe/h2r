//! Process-isolated x86-64 Linux write-watch probe. Only the probe owns
//! SIGSEGV. The game uses its checked page table and software dirty marks.
use std::io;
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

const PAGES: usize = 4;
const SEGV_ACCERR: i32 = 2; // Linux UAPI: protection failure.
static ACTIVE: AtomicPtr<Watch> = AtomicPtr::new(ptr::null_mut());

struct Watch {
    base: *mut u8,
    page_size: usize,
    tid: libc::pid_t,
    previous: libc::sigaction,
    dirty: AtomicUsize,
    hits: AtomicUsize,
    last_rip: AtomicUsize,
    last_addr: AtomicUsize,
}

impl Watch {
    fn page(&self, index: usize) -> *mut u8 {
        assert!(index < PAGES);
        self.base.wrapping_add(index * self.page_size)
    }

    fn written_page(&self, addr: usize, tid: libc::pid_t, trap: i64, error: i64) -> Option<usize> {
        // #PF, user-mode data write (U/S and W/R). Allow both values of
        // the present bit: a demand-zero page can be absent on first touch.
        if tid != self.tid || trap != 14 || !matches!(error, 0b110 | 0b111) {
            return None;
        }
        let index = addr.checked_sub(self.base as usize)? / self.page_size;
        // Once opened, a page cannot fault because of this watch again.
        (index < PAGES && self.dirty.load(Ordering::Relaxed) & (1 << index) == 0).then_some(index)
    }

    fn rearm(&self) -> io::Result<()> {
        // SAFETY: the scenario calls this with the writer quiescent.
        syscall(unsafe {
            libc::mprotect(self.base.cast(), self.page_size * PAGES, libc::PROT_READ)
        })?;
        self.dirty.store(0, Ordering::Relaxed);
        Ok(())
    }
}

fn syscall(ret: libc::c_int) -> io::Result<()> {
    if ret == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn current_action() -> io::Result<libc::sigaction> {
    let mut action = std::mem::MaybeUninit::zeroed();
    // SAFETY: a null new action requests only the current disposition.
    syscall(unsafe { libc::sigaction(libc::SIGSEGV, ptr::null(), action.as_mut_ptr()) })?;
    Ok(unsafe { action.assume_init() })
}

extern "C" fn segv(sig: libc::c_int, info: *mut libc::siginfo_t, ctx: *mut libc::c_void) {
    // SAFETY: with_watch publishes the Watch before installing this action,
    // restores the old action before dropping it, and joins any test thread
    // before returning. SA_SIGINFO supplies both records.
    let watch = unsafe { &*ACTIVE.load(Ordering::Acquire) };
    let info = unsafe { &*info };
    // si_addr is defined only for a hardware fault; a user-raised SIGSEGV
    // carries the sender instead. Read the union only on the matching path.
    if info.si_code == SEGV_ACCERR {
        let context = unsafe { &*ctx.cast::<libc::ucontext_t>() };
        let regs = &context.uc_mcontext.gregs;
        let addr = unsafe { info.si_addr() } as usize;
        let tid = unsafe { libc::syscall(libc::SYS_gettid) } as libc::pid_t;
        if let Some(index) = watch.written_page(
            addr,
            tid,
            regs[libc::REG_TRAPNO as usize],
            regs[libc::REG_ERR as usize],
        ) {
            let bit = 1 << index;
            let page = watch.page(index);
            // SAFETY: a page in this probe's live mapping. Use the Linux
            // syscall directly from the signal handler.
            if unsafe {
                libc::syscall(
                    libc::SYS_mprotect,
                    page,
                    watch.page_size,
                    libc::PROT_READ | libc::PROT_WRITE,
                )
            } == 0
            {
                watch.dirty.fetch_or(bit, Ordering::Relaxed);
                watch
                    .last_rip
                    .store(regs[libc::REG_RIP as usize] as usize, Ordering::Relaxed);
                watch.last_addr.store(addr, Ordering::Relaxed);
                watch.hits.fetch_add(1, Ordering::Relaxed);
                // sigreturn retries the original instruction at the saved RIP.
                return;
            }
        }
    }

    // This scenario is finished. Let the kernel redeliver a hardware fault
    // to the previous action with its original siginfo, PC, mask and flags.
    // A software signal needs an explicit re-raise on this thread.
    unsafe {
        if libc::sigaction(sig, &watch.previous, ptr::null_mut()) != 0
            || (info.si_code <= 0 && libc::raise(sig) != 0)
        {
            libc::_exit(128 + sig);
        }
    }
}

fn with_watch(run: fn(&Watch)) -> io::Result<()> {
    let previous = current_action()?;
    // SAFETY: no pointers for sysconf or gettid. The private anonymous pages
    // are zero-filled and the initial PROT_READ arms them for first writes.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    let page_size = usize::try_from(page_size)
        .ok()
        .filter(|p| p.is_power_of_two())
        .ok_or_else(|| io::Error::other("invalid host page size"))?;
    // One more read-only page belongs to the allocation but lies outside
    // the watch. The foreign-write scenario targets it.
    let size = page_size * (PAGES + 1);
    let base = unsafe {
        libc::mmap(
            ptr::null_mut(),
            size,
            libc::PROT_READ,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if base == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    let watch = Watch {
        base: base.cast(),
        page_size,
        tid: unsafe { libc::syscall(libc::SYS_gettid) } as libc::pid_t,
        previous,
        dirty: 0.into(),
        hits: 0.into(),
        last_rip: 0.into(),
        last_addr: 0.into(),
    };
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = segv as *const () as usize;
    action.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
    // SAFETY: action is initialized and the watch stays in place throughout
    // this scope. This binary installs only one watch at a time.
    unsafe { libc::sigemptyset(&mut action.sa_mask) };
    ACTIVE.store(ptr::from_ref(&watch).cast_mut(), Ordering::Release);
    struct Installed<'a>(&'a Watch, usize);
    impl Drop for Installed<'_> {
        fn drop(&mut self) {
            // SAFETY: restore before revoking access to the state or mapping.
            unsafe {
                if libc::sigaction(libc::SIGSEGV, &self.0.previous, ptr::null_mut()) != 0 {
                    std::process::abort();
                }
                ACTIVE.store(ptr::null_mut(), Ordering::Release);
                libc::munmap(self.0.base.cast(), self.1);
            }
        }
    }
    let _installed = Installed(&watch, size);
    syscall(unsafe { libc::sigaction(libc::SIGSEGV, &action, ptr::null_mut()) })?;
    run(&watch);
    Ok(())
}

fn smoke(watch: &Watch) {
    // SAFETY: mapped, aligned storage owned by this scenario. A first
    // store opens the page in segv; repeat writes go straight through.
    let p = watch.page(2).wrapping_add(8).cast::<u64>();
    let store_pc: usize;
    unsafe {
        std::arch::asm!(
            "lea {pc}, [rip + 2f]",
            "2: mov qword ptr [{p}], 41",
            pc = out(reg) store_pc, p = in(reg) p,
            options(nostack, preserves_flags),
        );
        p.write_volatile(42);
    }
    assert_eq!(unsafe { p.read_volatile() }, 42);
    assert_eq!(watch.hits.load(Ordering::Relaxed), 1);
    assert_eq!(watch.dirty.load(Ordering::Relaxed), 1 << 2);
    assert_eq!(watch.last_addr.load(Ordering::Relaxed), p as usize);
    assert_eq!(watch.last_rip.load(Ordering::Relaxed), store_pc);

    watch.rearm().unwrap();
    let cross = watch.page(1).wrapping_sub(3);
    // SAFETY: one machine store whose eight bytes span two armed pages.
    unsafe {
        std::arch::asm!("mov qword ptr [{p}], {v}", p = in(reg) cross,
        v = in(reg) 0x0807_0605_0403_0201u64, options(nostack, preserves_flags))
    };
    assert_eq!(watch.dirty.load(Ordering::Relaxed), 3);
    assert_eq!(watch.hits.load(Ordering::Relaxed), 3);
    assert_eq!(
        unsafe { cross.cast::<u64>().read_unaligned() },
        0x0807_0605_0403_0201
    );
}

pub(super) fn run(mode: &str) -> io::Result<()> {
    match mode {
        "all" => {
            run("smoke")?;
            run("teardown")?;
        }
        "smoke" => with_watch(smoke)?,
        "teardown" => {
            let initial = current_action()?.sa_sigaction;
            for _ in 0..5 {
                with_watch(smoke)?;
                assert_eq!(current_action()?.sa_sigaction, initial);
            }
        }
        "chain-handler" => {
            // Distinguish a real re-delivery to a previous action from a
            // generic process crash. This scenario runs in its own child.
            static FOREIGN: AtomicUsize = AtomicUsize::new(0);
            extern "C" fn previous(
                _: libc::c_int,
                info: *mut libc::siginfo_t,
                _: *mut libc::c_void,
            ) {
                let foreign = FOREIGN.load(Ordering::Acquire);
                let info = unsafe { &*info };
                let original = foreign != 0
                    && info.si_code == SEGV_ACCERR
                    && unsafe { info.si_addr() } as usize == foreign;
                unsafe { libc::_exit(if original { 42 } else { 1 }) };
            }
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction = previous as *const () as usize;
            action.sa_flags = libc::SA_SIGINFO;
            unsafe { libc::sigemptyset(&mut action.sa_mask) };
            syscall(unsafe { libc::sigaction(libc::SIGSEGV, &action, ptr::null_mut()) })?;
            with_watch(|w| unsafe {
                let outside = w.base.add(PAGES * w.page_size);
                FOREIGN.store(outside as usize, Ordering::Release);
                outside.write_volatile(1);
            })?;
        }
        "unregistered-write" => with_watch(|w| unsafe {
            w.base.add(PAGES * w.page_size).write_volatile(1);
        })?,
        "region-read-none" => with_watch(|w| unsafe {
            syscall(libc::mprotect(w.base.cast(), w.page_size, libc::PROT_NONE)).unwrap();
            std::arch::asm!("mov al, byte ptr [{p}]", p = in(reg) w.base, out("al") _, options(nostack));
        })?,
        "unattached-thread-write" => with_watch(|w| {
            let addr = w.base as usize;
            std::thread::spawn(move || unsafe {
                (addr as *mut u8).write_volatile(1);
            })
            .join()
            .unwrap();
        })?,
        "null-deref" => with_watch(|_| unsafe {
            std::arch::asm!("mov al, byte ptr [{p}]", p = in(reg) 0usize, out("al") _, options(nostack));
        })?,
        _ => return Err(io::Error::other(format!("unknown scenario {mode}"))),
    }
    println!("{mode} ok");
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn classifies_access_kind_owner_and_page_at_both_edges() {
        let w = Watch {
            base: 0x10000usize as *mut u8,
            page_size: 4096,
            tid: 7,
            previous: current_action().unwrap(),
            dirty: 2.into(),
            hits: 0.into(),
            last_rip: 0.into(),
            last_addr: 0.into(),
        };
        let check = |addr, tid, trap, err| w.written_page(addr, tid, trap, err);
        assert_eq!(check(0x10000, 7, 14, 0b110), Some(0));
        assert_eq!(check(0x13fff, 7, 14, 0b111), Some(3));
        assert_eq!(check(0x11000, 7, 14, 0b111), None); // open
        assert_eq!(check(0x0ffff, 7, 14, 0b111), None);
        assert_eq!(check(0x14000, 7, 14, 0b111), None);
        assert_eq!(check(0x10000, 8, 14, 0b111), None);
        assert_eq!(check(0x10000, 7, 13, 0b111), None);
        assert_eq!(check(0x10000, 7, 14, 0b101), None); // read
        assert_eq!(check(0x10000, 7, 14, 0b10111), None); // instruction fetch
    }
}
