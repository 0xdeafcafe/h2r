//! A fault hostfault does not own reaches the handler installed before it.
//! Its own test binary, so nothing has installed hostfault before the
//! sentinel below; each case runs in a forked child that the sentinel ends.
#![cfg(unix)]

use std::ptr;

const OUTSIDE: i32 = 42;

extern "C" fn sentinel(_: i32, _: *mut libc::siginfo_t, _: *mut libc::c_void) {
    // SAFETY: async-signal-safe.
    unsafe { libc::_exit(OUTSIDE) }
}

fn decline(_: &mut hostfault::Fault, _: usize) -> bool {
    false
}

/// A page nobody may touch.
fn guard_page() -> *mut u8 {
    // SAFETY: an anonymous PROT_NONE mapping, never freed (the child exits).
    let p = unsafe {
        libc::mmap(
            ptr::null_mut(),
            1 << 16,
            libc::PROT_NONE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        )
    };
    assert_ne!(p, libc::MAP_FAILED);
    p.cast()
}

/// Run `child` in a forked process and return its exit status.
fn in_child(child: impl FnOnce()) -> i32 {
    // SAFETY: the child only maps memory, sets handlers and faults.
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0);
    if pid == 0 {
        child();
        // SAFETY: async-signal-safe; the fault above should not return.
        unsafe { libc::_exit(1) }
    }
    let mut status = 0;
    // SAFETY: our own child.
    assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
    assert!(
        libc::WIFEXITED(status),
        "child died of a signal: {status:#x}"
    );
    libc::WEXITSTATUS(status)
}

fn install_sentinel() {
    let signals: &[i32] = if cfg!(target_vendor = "apple") {
        &[libc::SIGBUS, libc::SIGSEGV]
    } else {
        &[libc::SIGSEGV]
    };
    for &sig in signals {
        // SAFETY: a zeroed sigaction filled with a three-argument handler.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = sentinel as *const () as usize;
            action.sa_flags = libc::SA_SIGINFO;
            assert_eq!(libc::sigaction(sig, &action, ptr::null_mut()), 0);
        }
    }
}

#[test]
fn unowned_fault_reaches_the_previous_handler() {
    // A fault at an address in no region.
    let status = in_child(|| {
        install_sentinel();
        let owned = guard_page();
        let elsewhere = guard_page();
        let _region = hostfault::register_region(owned as usize, 1 << 16, hostfault::fixup, 0)
            .expect("install");
        // SAFETY: deliberately faults; the sentinel ends the child.
        unsafe { ptr::read_volatile(elsewhere) };
    });
    assert_eq!(status, OUTSIDE, "address outside every region");

    // A fault in a region whose callback finds no fixup row for the PC.
    let status = in_child(|| {
        install_sentinel();
        let owned = guard_page();
        let _region = hostfault::register_region(owned as usize, 1 << 16, hostfault::fixup, 0)
            .expect("install");
        // SAFETY: as above.
        unsafe { ptr::read_volatile(owned.add(8)) };
    });
    assert_eq!(status, OUTSIDE, "PC with no row");

    // A fault every callback declines.
    let status = in_child(|| {
        install_sentinel();
        let owned = guard_page();
        let _region =
            hostfault::register_region(owned as usize, 1 << 16, decline, 0).expect("install");
        // SAFETY: as above.
        unsafe { ptr::write_volatile(owned, 1) };
    });
    assert_eq!(status, OUTSIDE, "declined");
}
