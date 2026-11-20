//! Run in child processes so the parent keeps its signal dispositions.
//! Actual host store instructions and the production Rust guard/epoch.
use super::*;
use crate::guest::Permissions;
use crate::watch::CONTIGUOUS_BASE;
use std::process::Command;
use std::ptr;
use std::sync::atomic::AtomicUsize;

static FOREIGN_ADDRESS: AtomicUsize = AtomicUsize::new(0);

const VA: u32 = 0x0100_0000;
const ALIAS: u32 = 0x0200_0000;
const PAGE: u32 = 4096;
#[cfg(target_os = "linux")]
const SIGNAL: i32 = libc::SIGSEGV;
#[cfg(target_os = "macos")]
const SIGNAL: i32 = libc::SIGBUS;

fn store(memory: &Memory, va: u32, value: u8) {
    assert!(memory.try_gate());
    // SAFETY: this models generated C with an exclusive gate and a live
    // view. Require the experimental path to be available for this test.
    unsafe {
        let d = &*memory.shared.data.get();
        assert_ne!(d.watching >> 31, 0);
        let p = *d.pages.add((va >> 12) as usize);
        assert_eq!(
            p.permissions & (crate::flat::FLAT | 2),
            crate::flat::FLAT | 2
        );
        let pa = p.physical | (va & 4095);
        let plane = (pa >> 12) & d.flat.plane_mask;
        assert!(!d.flat.stores.is_null());
        d.flat
            .stores
            .add(((plane as usize) << 26) + pa as usize)
            .write_volatile(value);
        assert_eq!(d.flat.base.add(va as usize).read_volatile(), value);
    }
    memory.release_gate();
}

#[test]
fn flat_faults_and_exact_watch_epochs() {
    const KEY: &str = "H2_TEST_FLAT_FAULT_CHILD";
    let Some(mode) = std::env::var_os(KEY) else {
        for (mode, expected) in [("watch", 0), ("foreign", 79), ("siginfo", 81)] {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "ffi::memory::flat_tests::flat_faults_and_exact_watch_epochs",
                    "--nocapture",
                ])
                .env(KEY, mode)
                .env("H2_FLAT_RAM", "1")
                .env("H2_FLAT_WRITE_WATCH", "1")
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(expected),
                "{mode}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("running 1 test"));
        }
        return;
    };

    if mode != "watch" {
        extern "C" fn previous(_: i32) {
            unsafe { libc::_exit(79) }
        }
        extern "C" fn with_info(sig: i32, info: *mut libc::siginfo_t, context: *mut libc::c_void) {
            let addr = unsafe { (*info).si_addr() } as usize;
            let exact = sig == SIGNAL
                && !context.is_null()
                && addr == FOREIGN_ADDRESS.load(Ordering::Relaxed);
            unsafe { libc::_exit(if exact { 81 } else { 82 }) }
        }
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            if mode == "siginfo" {
                action.sa_sigaction = with_info as *const () as usize;
                action.sa_flags = libc::SA_SIGINFO;
            } else {
                action.sa_sigaction = previous as *const () as usize;
            }
            libc::sigemptyset(&mut action.sa_mask);
            assert_eq!(libc::sigaction(SIGNAL, &action, ptr::null_mut()), 0);
        }
    }
    let mut g = Guest::new();
    for addr in [VA, ALIAS] {
        g.map(addr, 4 * PAGE, 0, Permissions::RW).unwrap();
    }
    let a = g.subscribe_writes("all");
    let b = g.subscribe_writes("late");
    g.watch_writes(a, CONTIGUOUS_BASE, 4 * PAGE).unwrap();
    let memory = Memory::new(g);
    memory.fast_shared();

    if mode != "watch" {
        // Same protection fault as the write barrier, outside all mirrors.
        assert!(memory.try_gate());
        unsafe {
            let d = &*memory.shared.data.get();
            assert!(!d.flat.stores.is_null());
            let n = libc::sysconf(libc::_SC_PAGESIZE) as usize;
            let p = libc::mmap(
                ptr::null_mut(),
                n,
                libc::PROT_READ,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            );
            assert_ne!(p, libc::MAP_FAILED);
            FOREIGN_ADDRESS.store(p as usize, Ordering::Relaxed);
            p.cast::<u8>().write_volatile(1);
        }
        panic!("unowned fault was swallowed");
    }

    {
        let mut other = Guest::new();
        other.map(VA, 4 * PAGE, 0, Permissions::RW).unwrap();
        let id = other.subscribe_writes("second guest");
        other.watch_writes(id, CONTIGUOUS_BASE, PAGE).unwrap();
        let second = Memory::new(other);
        second.fast_shared();
        store(&second, VA, 101);
        assert!(second.lock().has_dirty(id));
        assert_eq!(memory.lock().u8(VA).unwrap(), 0);
        assert!(!memory.lock().has_dirty(a));
    }
    store(&memory, VA + PAGE, 0); // same-value store still dirties
    store(&memory, ALIAS + PAGE + 1, 11);
    store(&memory, ALIAS + 3 * PAGE, 33);
    let range = |i: u32| crate::DirtyRange {
        addr: CONTIGUOUS_BASE + i * PAGE,
        len: PAGE,
    };
    let frozen = {
        let mut g = memory.lock();
        assert_eq!(g.u8(VA + PAGE + 1).unwrap(), 11);
        assert_eq!(g.take_dirty(a).unwrap(), [range(1), range(3)]);
        assert!(g.take_dirty(b).unwrap().is_empty());
        g.watch_writes(b, CONTIGUOUS_BASE + PAGE, PAGE).unwrap();
        g.freeze_ram().0
    };
    store(&memory, ALIAS + PAGE + 1, 22);
    assert!(memory.lock().has_dirty(b));
    assert_eq!(memory.lock().take_dirty(a).unwrap(), [range(1)]);
    store(&memory, VA + PAGE + 1, 44);
    assert_eq!(memory.lock().take_dirty(a).unwrap(), [range(1)]);
    assert_eq!(memory.lock().take_dirty(b).unwrap(), [range(1)]);
    assert_eq!(frozen.bytes()[PAGE as usize + 1], 11);

    {
        let mut g = memory.lock();
        g.install_trap(16);
        g.arm_all(true, true);
    }
    assert!(memory.try_gate());
    assert_eq!(unsafe { (*memory.shared.data.get()).watching } >> 31, 0);
    memory.release_gate();
    {
        let mut g = memory.lock();
        g.remove_trap();
        g.protect_range(VA + PAGE, PAGE, Permissions::READ).unwrap();
        g.unmap(VA + 2 * PAGE, PAGE).unwrap();
    }
    assert!(memory.try_gate());
    unsafe {
        let d = &*memory.shared.data.get();
        assert_eq!((*d.pages.add((VA / PAGE + 1) as usize)).permissions & 2, 0);
        assert_eq!((*d.pages.add((VA / PAGE + 2) as usize)).permissions, 0);
        assert!(!d.flat.base.is_null());
    }
    memory.release_gate();
    store(&memory, ALIAS + PAGE, 55);
    {
        let mut g = memory.lock();
        assert_eq!(g.u8(VA + PAGE).unwrap(), 55);
        let layout = g.layout();
        g.restore(&layout, frozen.bytes()).unwrap();
        assert_eq!(
            g.take_dirty(a).unwrap(),
            [crate::DirtyRange {
                addr: CONTIGUOUS_BASE,
                len: 4 * PAGE
            }]
        );
    }
    store(&memory, ALIAS + PAGE, 66);
    assert_eq!(memory.lock().take_dirty(a).unwrap(), [range(1)]);

    // Replacing the RAM backing must replace its watched-store mirror too.
    let path = std::env::temp_dir().join(format!("h2-flat-restore-{}", std::process::id()));
    let image = std::fs::File::create_new(&path).unwrap();
    image.set_len(crate::guest::RAM_SIZE as u64).unwrap();
    {
        let mut g = memory.lock();
        let layout = g.layout();
        g.restore_cow(&layout, &path).unwrap();
        assert_eq!(g.u8(ALIAS + PAGE).unwrap(), 0);
    }
    std::fs::remove_file(path).unwrap();
    assert!(memory.try_gate());
    // SAFETY: the gate pins the same pointers used by a generated store.
    let mirrored = unsafe {
        let d = &*memory.shared.data.get();
        #[cfg(target_os = "linux")]
        assert_eq!(d.watching >> 31, 0);
        d.watching >> 31 != 0 && !d.flat.stores.is_null()
            && (*d.pages.add((ALIAS / PAGE + 1) as usize)).permissions & crate::flat::FLAT != 0
    };
    memory.release_gate();
    if mirrored {
        store(&memory, ALIAS + PAGE, 88);
        assert_eq!(memory.lock().u8(ALIAS + PAGE).unwrap(), 88);
    }
}
