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

/// Issue the same byte store as generated code. Returns true if it used
/// the faulting mirror. The native emitter fixture exercises the C decision.
fn store(memory: &Memory, va: u32, value: u8) -> bool {
    assert!(memory.try_gate());
    let mirrored = unsafe {
        let d = &mut *memory.shared.data.get();
        let p = *d.pages.add((va >> 12) as usize);
        assert_ne!(p.permissions & 2, 0);
        let pa = p.physical | (va & 4095);
        let page = pa as usize >> 12;
        let bit = 1u64 << (page % 64);
        let mut stores = ptr::null_mut();
        if d.watching >> 31 != 0 {
            stores = d.flat.stores;
            if stores.is_null()
                && d.watching & (1 << 30) != 0
                && *d.flat.software_pages.add(page / 64) & bit == 0
            {
                stores = d.flat.cold_stores;
            }
        }
        if stores.is_null() {
            d.ram.add(pa as usize).write_volatile(value);
            if d.watching & 1 != 0 {
                d.dirty[page / 64] |= bit;
                d.dirty_lo = d.dirty_lo.min((page / 64) as u32);
                d.dirty_hi = d.dirty_hi.max((page / 64) as u32 + 1);
            }
        } else {
            let plane = (page as u32 & d.flat.plane_mask) as usize;
            stores
                .add((plane << 26) + pa as usize)
                .write_volatile(value);
        }
        assert_eq!(d.ram.add(pa as usize).read_volatile(), value);
        if d.watching >> 31 != 0 && p.permissions & crate::flat::FLAT != 0 {
            assert_eq!(d.flat.base.add(va as usize).read_volatile(), value);
        }
        !stores.is_null()
    };
    memory.release_gate();
    mirrored
}

fn hot_epochs() {
    let mut g = Guest::new();
    for va in [VA, ALIAS] {
        g.map(va, 4 * PAGE, 0, Permissions::RW).unwrap();
    }
    let a = g.subscribe_writes("each tick");
    let b = g.subscribe_writes("every fourth tick");
    for id in [a, b] {
        g.watch_writes(id, CONTIGUOUS_BASE, 4 * PAGE).unwrap();
    }
    let m = Memory::new(g);
    m.fast_shared();
    let range = |page: u32| crate::DirtyRange {
        addr: CONTIGUOUS_BASE + page * PAGE,
        len: PAGE,
    };
    for tick in 0..16 {
        let va = if tick % 2 == 0 { VA } else { ALIAS };
        // Two first-write epochs through either VA, then the software barrier.
        assert_eq!(store(&m, va + PAGE, 7), tick < 2);
        let mut g = m.lock();
        assert_eq!(g.u8(ALIAS + PAGE).unwrap(), 7);
        assert_eq!(g.take_dirty(a).unwrap(), [range(1)]);
        if tick % 4 == 3 {
            assert_eq!(g.take_dirty(b).unwrap(), [range(1)]);
        }
    }
    assert!(store(&m, VA + 2 * PAGE, 9), "a cold sibling still faults");
    assert_eq!(m.lock().take_dirty(a).unwrap(), [range(2)]);
    assert_eq!(m.lock().take_dirty(b).unwrap(), [range(2)]);
}

fn file_restore_with_live_mirror() {
    use std::fs::{self, File};
    use std::io::Write;
    let path = std::env::temp_dir().join(format!("h2-flat-restore-{}", std::process::id()));
    let mut file = File::create_new(&path).unwrap();
    file.set_len(crate::guest::RAM_SIZE as u64).unwrap();
    file.write_all(&[12]).unwrap();
    drop(file);
    let guest = || {
        let mut g = Guest::new();
        for va in [VA, ALIAS] {
            g.map(va, PAGE, 0, Permissions::RW).unwrap();
        }
        let id = g.subscribe_writes("private");
        g.watch_writes(id, CONTIGUOUS_BASE, PAGE).unwrap();
        let m = Memory::new(g);
        m.fast_shared();
        (m, id)
    };
    let (first, id) = guest();
    let (other, other_id) = guest();
    let layout = first.lock().layout();
    {
        let (view, _) = guest();
        for value in [20, 21] {
            store(&first, ALIAS, value);
            view.lock().cow_refresh(&first.lock());
            assert_eq!(view.lock().u8(VA).unwrap(), value);
            assert!(!store(&view, VA, 90));
            assert_eq!(first.lock().u8(ALIAS).unwrap(), value);
            store(&first, VA, 80);
            assert_eq!(view.lock().u8(ALIAS).unwrap(), 90);
        }
    }
    for (m, w) in [(&first, id), (&other, other_id)] {
        assert!(store(m, VA, 42));
        assert!(
            m.lock()
                .restore_cow(&layout, &path.with_extension("absent"))
                .is_err()
        );
        assert!(store(m, ALIAS, 43));
        let mut g = m.lock();
        g.restore_cow(&layout, &path).unwrap();
        assert_eq!(g.u8(ALIAS).unwrap(), 12);
        assert_eq!(g.take_dirty(w).unwrap().len(), 1);
    }
    // Even with the old canonical address (Linux MAP_FIXED), no old
    // mirror or memfd translation may be returned to generated code.
    assert!(!store(&first, ALIAS, 73));
    assert!(first.lock().has_dirty(id));
    assert_eq!(first.lock().u8(VA).unwrap(), 73);
    assert_eq!(other.lock().u8(VA).unwrap(), 12);
    assert!(!other.lock().has_dirty(other_id));
    assert_eq!(first.lock().cow_census().unwrap().unwrap().0, 1);
    assert_eq!(other.lock().cow_census().unwrap().unwrap().0, 0);
    first.lock().restore_cow(&layout, &path).unwrap();
    assert_eq!(first.lock().u8(VA).unwrap(), 12);
    fs::remove_file(path).unwrap();
}

#[test]
fn flat_faults_and_exact_watch_epochs() {
    const KEY: &str = "H2_TEST_FLAT_FAULT_CHILD";
    let Some(mode) = std::env::var_os(KEY) else {
        for (mode, expected) in [
            ("watch", 0),
            ("hot", 0),
            ("cow", 0),
            ("foreign", 79),
            ("siginfo", 81),
        ] {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "ffi::memory::flat_tests::flat_faults_and_exact_watch_epochs",
                    "--nocapture",
                ])
                .env(KEY, mode)
                .env("H2_FLAT_RAM", "1")
                .env("H2_FLAT_WRITE_WATCH", "1")
                .env("H2_FLAT_STATS", "1")
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(expected),
                "{mode}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("running 1 test"));
            if mode == "hot" {
                let stderr = String::from_utf8_lossy(&output.stderr);
                assert!(
                    stderr.contains(
                        "faults=3 retries=3 re-protects=2 pages-armed=6 software-pages=1"
                    ),
                    "{stderr}"
                );
            }
        }
        return;
    };

    if mode == "cow" {
        return file_restore_with_live_mirror();
    }
    if mode == "hot" {
        return hot_epochs();
    }

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
}
