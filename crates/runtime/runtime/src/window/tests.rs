use super::*;
use crate::guest::{Permissions, RAM_SIZE};
use crate::watch::{CONTIGUOUS_BASE, DirtyRange};
use crate::{CopyKind, Dispatch, Guest, Runtime};

/// D2 of the contract change: after a copy-on-write copy of the live RAM,
/// a store through the window is seen through the canonical mapping and
/// the reverse, and neither reaches the copy.
#[test]
fn alias_tracks_canonical_after_cow_copy() {
    const LEN: usize = 1 << 20;
    let mut ram = Ram::zeroed_with(LEN, true);
    let w = Window::new().expect("a window on this host");
    assert!(os::can_alias(&ram, w.host_page()), "an aliasable backing");
    let at = w.base();
    // SAFETY: `at` is the window's own reservation, host-page aligned.
    assert!(unsafe { ram.alias(at, 0, LEN) });
    ram[0x100] = 1;
    let (copy, _) = ram.cow_copy();
    let canonical = ram.as_mut_ptr();
    // SAFETY: both pointers cover LEN live bytes of the same RAM; nothing
    // else touches it during the test.
    unsafe {
        at.add(0x2000).write_volatile(7);
        assert_eq!(canonical.add(0x2000).read_volatile(), 7, "window to canonical");
        canonical.add(0x3000).write_volatile(9);
        assert_eq!(at.add(0x3000).read_volatile(), 9, "canonical to window");
        at.add(0x100).write_volatile(2);
        assert_eq!(canonical.add(0x100).read_volatile(), 2);
    }
    assert_eq!((copy[0x100], copy[0x2000], copy[0x3000]), (1, 0, 0), "the copy keeps its moment");
}

/// The window's base is the runtime's for good: a Guest put in place of
/// another is shown at the same base, and the one replaced no longer
/// changes the window.
#[test]
fn window_survives_guest_replacement() {
    let rt = Runtime::new(Guest::new_aliasable(), Dispatch::default());
    let base = rt.flat_base().expect("a window on this host");
    {
        let mut m = rt.memory();
        m.map(0x10000, 0x4000, 0x8000, Permissions::RW).unwrap();
        m.set_u32(0x10010, 0x1111).unwrap();
    }
    let w = rt.memory().window().cloned().expect("the runtime's window");
    assert!(w.is_aliased());
    let read = |va: usize| {
        // SAFETY: only called for pages the window holds (checked first).
        unsafe { (base.add(va) as *const u32).read_volatile() }
    };
    assert!(w.holds(0x10010));
    assert_eq!(read(0x10010), 0x1111);

    let mut next = Guest::new_aliasable();
    next.map(0x20000, 0x4000, 0x4000, Permissions::RW).unwrap();
    next.set_u32(0x20020, 0x2222).unwrap();
    let mut old = std::mem::replace(&mut *rt.memory(), next);
    assert_eq!(rt.flat_base(), Some(base));
    assert!(w.holds(0x20020));
    assert!(!w.holds(0x10010), "the replaced Guest's pages are gone");
    assert_eq!(read(0x20020), 0x2222);

    old.map(0x30000, 0x4000, 0xc000, Permissions::RW).unwrap();
    assert!(!w.holds(0x30000), "the replaced Guest no longer touches the window");
    assert_eq!(rt.window_stats().pages_outside, 0);
}

/// A page whose backing is not congruent with its address modulo the host
/// page, or that the access trap has armed, is a guard; fresh allocations
/// are congruent modulo 16 KiB on every host.
#[test]
fn non_congruent_page_is_guarded() {
    let rt = Runtime::new(Guest::new_aliasable(), Dispatch::default());
    let w = rt.memory().window().cloned().expect("a window on this host");
    let host = w.host_page();
    {
        let mut m = rt.memory();
        m.map(0x40000, 0x1000, 0x1000, Permissions::RW).unwrap();
        m.map(0x50000, 0x1000, 0x10000, Permissions::RW).unwrap();
        m.map(0x60000, 0x1000, 0x20000, Permissions::RW).unwrap();
        m.install_trap(16);
        m.set_sticky_pages(&[0x60]);
    }
    assert!(w.holds(0x50000));
    assert!(!w.holds(0x60000), "an armed page is a guard");
    let non_congruent = if host == 16384 {
        assert!(!w.holds(0x40000), "one page off in a 16 KiB host page");
        1
    } else {
        assert!(w.holds(0x40000), "every page is congruent with a 4 KiB host page");
        0
    };
    assert_eq!(rt.window_stats().pages_outside, non_congruent + 1);

    rt.memory().remove_trap();
    assert!(w.holds(0x60000), "disarmed, the page is back");
    assert_eq!(rt.window_stats().pages_outside, non_congruent);

    // Never-used RAM starts at 0x21000; residue 0 is 12 KiB on.
    let skipped = rt.window_stats().congruence_skipped;
    rt.memory().map_zeroed(0x70000, 0x2000, Permissions::RW).unwrap();
    let physical = rt
        .memory()
        .mappings()
        .iter()
        .find(|m| m.virtual_start == 0x70000)
        .unwrap()
        .physical_start;
    assert_eq!(physical, 0x24000);
    assert_eq!(rt.window_stats().congruence_skipped - skipped, 0x3000);
    assert!(w.holds(0x70000) && w.holds(0x71000));
    // Reuse of a freed gap is congruent too, where it can be.
    let gap = rt.memory().find_free_physical_for(0x9000_1000, 0x1000, 0..RAM_SIZE as u32);
    assert_eq!(gap.map(|p| p % CONGRUENCE), Some(0x1000));
}

fn watched(g: &mut Guest, owner: &'static str) -> crate::WatchId {
    let id = g.subscribe_writes(owner);
    g.watch_writes(id, CONTIGUOUS_BASE, RAM_SIZE as u32).unwrap();
    id
}

fn pages(list: &[u32]) -> Vec<DirtyRange> {
    list.iter()
        .map(|&p| DirtyRange {
            addr: CONTIGUOUS_BASE | p,
            len: 0x1000,
        })
        .collect()
}

/// A written card reports the physical page behind its guest page and
/// behind the guest page after it, through whatever mappings cover them,
/// and nothing else; a fold cleans the cards; a card written while no
/// watch is active is dropped.
#[test]
fn cards_fold_to_exact_pages() {
    let w = Window::new().expect("a window on this host");
    let mut g = Guest::new();
    g.attach_window(w.clone());
    g.map(0x10000, 0x2000, 0x8000, Permissions::RW).unwrap();
    g.map(0x40000, 0x1000, 0x3000, Permissions::RW).unwrap();
    g.map(0x50000, 0x1000, 0x6000, Permissions::RW).unwrap();
    g.map(0x51000, 0x1000, 0xc000, Permissions::RW).unwrap();
    let id = watched(&mut g, "cards");
    w.mark(0x10ffc); // 0x8000, and 0x9000 after it
    w.mark(0x40000); // 0x3000; the page after is unmapped
    w.mark(0x50ffe); // 0x6000, and 0xc000 behind the next guest page
    assert!(g.has_dirty(id), "unfolded cards count");
    assert_eq!(
        g.take_dirty(id).unwrap(),
        vec![
            DirtyRange { addr: CONTIGUOUS_BASE | 0x3000, len: 0x1000 },
            DirtyRange { addr: CONTIGUOUS_BASE | 0x6000, len: 0x1000 },
            DirtyRange { addr: CONTIGUOUS_BASE | 0x8000, len: 0x2000 },
            DirtyRange { addr: CONTIGUOUS_BASE | 0xc000, len: 0x1000 },
        ]
    );
    assert!(!g.has_dirty(id));
    assert!(g.take_dirty(id).unwrap().is_empty(), "the fold cleaned the cards");

    assert!(g.unsubscribe_writes(id));
    w.mark(0x10000);
    let id = watched(&mut g, "again");
    assert!(!g.has_dirty(id), "a store with no watch active is not reported later");
    assert!(g.take_dirty(id).unwrap().is_empty());
}

/// Cards are folded before a mute changes and before a drain: a store
/// before the mute reaches the muted subscriber, one inside it does not,
/// and every subscriber sees one made before its drain.
#[test]
fn fold_before_mute_and_drain() {
    let w = Window::new().expect("a window on this host");
    let mut g = Guest::new();
    g.attach_window(w.clone());
    g.map(0x10000, 0x1000, 0x4000, Permissions::RW).unwrap();
    g.map(0x20000, 0x1000, 0x8000, Permissions::RW).unwrap();
    g.map(0x30000, 0x1000, 0xc000, Permissions::RW).unwrap();
    let (a, b) = (watched(&mut g, "a"), watched(&mut g, "b"));
    w.mark(0x10000);
    g.writes_unreported(a, |_| w.mark(0x20000)).unwrap();
    w.mark(0x30000);
    assert_eq!(g.take_dirty(a).unwrap(), pages(&[0x4000, 0xc000]));
    assert_eq!(g.take_dirty(b).unwrap(), pages(&[0x4000, 0x8000, 0xc000]));
}

/// A dispatch with a flat-RAM module's (empty) fixup table.
fn flat_dispatch() -> Dispatch {
    let mut d = Dispatch::default();
    // SAFETY: an empty table reads nothing.
    let table = unsafe {
        hostfault::register_table(std::ptr::null(), std::ptr::null(), hostfault::Format::Relative32)
    }
    .expect("an empty table");
    d.set_flat_fixups(table);
    d
}

/// Contract slice F3: a runtime with a flat-RAM module moves RAM made
/// before it into a backing the window can alias, bytes and mappings
/// unchanged, parks generated code, and points a context's block at the
/// window, or at the null window while fast memory is off.
#[test]
fn flat_runtime_aliases_ram_made_before_it() {
    let mut g = Guest::new();
    g.map(0x10000, 0x4000, 0x8000, Permissions::RW).unwrap();
    g.set_u32(0x10010, 0x1111).unwrap();
    let rt = Runtime::new(g, flat_dispatch());
    assert!(rt.flat_ram());
    assert!(rt.memory_state().parking(), "parking is on");
    let w = rt.memory().window().cloned().expect("a window on this host");
    let base = rt.flat_base().expect("a window");
    assert!(w.is_aliased(), "RAM aliased in");
    assert!(w.holds(0x10010));
    // SAFETY: the window holds the page (checked above).
    assert_eq!(unsafe { (base.add(0x10010) as *const u32).read_volatile() }, 0x1111);
    assert_eq!(rt.flat_block_target(true), (base, w.cards()));
    let null = null_window().expect("the null window");
    assert_eq!(rt.flat_block_target(false).0, null.base());
    assert_ne!(null.base(), base);

    // A Guest put in its place is moved too.
    let mut next = Guest::new();
    next.map(0x20000, 0x4000, 0x4000, Permissions::RW).unwrap();
    next.set_u32(0x20020, 0x2222).unwrap();
    drop(std::mem::replace(&mut *rt.memory(), next));
    assert!(w.is_aliased());
    assert!(w.holds(0x20020));
    // SAFETY: as above.
    assert_eq!(unsafe { (base.add(0x20020) as *const u32).read_volatile() }, 0x2222);
}

/// A paged runtime never parks or moves RAM, and a later flat module
/// switches both on through `edit_dispatch`.
#[test]
fn flat_module_registered_later_adopts_flat_ram() {
    let mut g = Guest::new();
    g.map(0x10000, 0x4000, 0x8000, Permissions::RW).unwrap();
    g.set_u32(0x10010, 0x3333).unwrap();
    let rt = Runtime::new(g, Dispatch::default());
    assert!(!rt.flat_ram());
    assert!(!rt.memory_state().parking());
    rt.edit_dispatch(|d| {
        // SAFETY: as in flat_dispatch.
        let table = unsafe {
            hostfault::register_table(std::ptr::null(), std::ptr::null(), hostfault::Format::Relative32)
        }
        .expect("an empty table");
        d.set_flat_fixups(table);
    });
    assert!(rt.flat_ram());
    assert!(rt.memory_state().parking());
    let w = rt.memory().window().cloned().expect("a window on this host");
    assert!(w.is_aliased());
    let base = rt.flat_base().expect("a window");
    // SAFETY: the window holds the page once aliased.
    assert!(w.holds(0x10010));
    assert_eq!(unsafe { (base.add(0x10010) as *const u32).read_volatile() }, 0x3333);
}

/// `Ram::make_aliasable` keeps every byte, zero pages included.
#[test]
fn make_aliasable_keeps_the_bytes() {
    let mut ram = Ram::zeroed_with(RAM_SIZE, false);
    ram[5] = 1;
    ram[RAM_SIZE - 1] = 2;
    ram[0x12345] = 3;
    assert!(ram.make_aliasable());
    assert!(ram.can_alias());
    assert_eq!((ram[5], ram[RAM_SIZE - 1], ram[0x12345], ram[0x5000]), (1, 2, 3, 0));
}

/// Contract slice F4: a flat-RAM runtime lends no paged view, so its
/// guards never take the gate or drain the C dirty bitmap; a store through
/// the window reaches the write watch through its card alone.
#[test]
fn flat_runtime_lends_no_paged_view() {
    let rt = Runtime::new(Guest::new(), flat_dispatch());
    let id = {
        let mut m = rt.memory();
        m.map_zeroed(0x10000, 0x4000, Permissions::RW).unwrap();
        watched(&mut m, "f4")
    };
    let mut ctx = crate::ExecCtx::new(rt.clone(), 0);
    let exec = (&mut ctx as *mut crate::ExecCtx).cast();
    // SAFETY: the context is live and nothing else uses it.
    assert!(unsafe { crate::ffi::h2abi_v1_fast_acquire(exec) }.is_null(), "no paged view");
    let w = rt.memory().window().cloned().expect("a window on this host");
    assert!(w.holds(0x10010));
    // A store through the window, then its card, as generated code makes it.
    // SAFETY: the window holds the page (checked above).
    unsafe { (rt.flat_base().unwrap().add(0x10010) as *mut u32).write_volatile(7) };
    w.mark(0x10010);
    let physical = rt
        .memory()
        .mappings()
        .iter()
        .find(|m| m.virtual_start == 0x10000)
        .unwrap()
        .physical_start;
    let dirty = rt.memory().take_dirty(id).unwrap();
    assert!(
        dirty.iter().any(|r| (r.addr..r.addr + r.len).contains(&(CONTIGUOUS_BASE | physical))),
        "{dirty:x?}"
    );
    assert_eq!(rt.memory().u32(0x10010).unwrap(), 7);
}

/// Contract slice F5: a second executor's view in a flat-RAM runtime has
/// its copy aliased into its own window at every refresh, under a new
/// layout or an unchanged one. The view's window reads the snapshot, a
/// store through it is the view's alone, and the live Guest's later
/// stores reach the view only at the next refresh. On macOS the copy is
/// the remap itself, not a second copy.
#[test]
fn view_refresh_realiases() {
    let live = Runtime::new(Guest::new(), flat_dispatch());
    live.memory().map_zeroed(0x10000, 0x8000, Permissions::RW).unwrap();
    live.memory().set_u32(0x10010, 0x1111).unwrap();
    let view = Runtime::new(Guest::new(), flat_dispatch());
    let w = view.memory().window().cloned().expect("a window on this host");
    let base = view.flat_base().expect("a window");
    let live_base = live.flat_base().expect("a window");
    // SAFETY: only called for pages the window holds (checked first).
    let read = |base: *mut u8, va: u32| unsafe { (base.add(va as usize) as *const u32).read_volatile() };
    for round in 0..3u32 {
        let kind = {
            let mut v = view.memory();
            let l = live.memory();
            v.cow_refresh(&l)
        };
        if cfg!(target_os = "macos") {
            assert_eq!(kind, CopyKind::Remapped, "round {round}");
        }
        assert!(w.is_aliased(), "round {round}: the copy is aliased");
        assert_eq!(view.window_stats().pages_outside, 0, "round {round}");
        assert!(w.holds(0x10010) && w.holds(0x14020));
        assert_eq!(read(base, 0x10010), 0x1111 + round, "round {round}: the snapshot");
        // A store through the view's window is the view's.
        // SAFETY: the window holds the page.
        unsafe { (base.add(0x14020) as *mut u32).write_volatile(0xa0 + round) };
        assert_eq!(view.memory().u32(0x14020).unwrap(), 0xa0 + round);
        assert_eq!(live.memory().u32(0x14020).unwrap(), 0, "round {round}: the view's store stays in the view");
        assert_eq!(read(live_base, 0x14020), 0);
        // A live store after the refresh stays out of the view.
        live.memory().set_u32(0x10010, 0x1111 + round + 1).unwrap();
        assert_eq!(read(live_base, 0x10010), 0x1111 + round + 1);
        assert_eq!(read(base, 0x10010), 0x1111 + round, "round {round}: the live store reached the view");
        if round == 1 {
            // A new layout for the last round: the window is rebuilt from it.
            live.memory().map_zeroed(0x40000, 0x4000, Permissions::RW).unwrap();
            live.memory().set_u32(0x40040, 0x4444).unwrap();
        }
    }
    assert!(w.holds(0x40040));
    assert_eq!(read(base, 0x40040), 0x4444);
}

/// Contract slice F5: a copy-on-write restore in a flat-RAM runtime is
/// aliased into the window. Through the window and through Rust each
/// guest sees its own stores, no other guest's, and the image is never
/// written. On macOS the window aliases the private file mapping itself
/// (the clean pages stay shared); on Linux the image is copied (Q1).
#[test]
fn restore_cow_keeps_window_coherent() {
    let mut source = Guest::new();
    source.map_zeroed(0x10000, 0x8000, Permissions::RW).unwrap();
    source.set_u32(0x10010, 0x1234_5678).unwrap();
    let layout = source.layout();
    let path = std::env::temp_dir().join(format!("h2-window-restore-{}.ram", std::process::id()));
    std::fs::write(&path, source.physical_ram()).unwrap();
    fn physical(rt: &Runtime) -> usize {
        rt.memory().mappings().iter().find(|m| m.virtual_start == 0x10000).unwrap().physical_start as usize
    }
    fn read(rt: &Runtime, va: u32) -> u32 {
        // SAFETY: only called for pages the window holds (checked first).
        unsafe { (rt.flat_base().unwrap().add(va as usize) as *const u32).read_volatile() }
    }
    let (a, b) = (Runtime::new(Guest::new(), flat_dispatch()), Runtime::new(Guest::new(), flat_dispatch()));
    for rt in [&a, &b] {
        rt.memory().restore_cow(&layout, &path).unwrap();
        let w = rt.memory().window().cloned().expect("a window on this host");
        assert!(w.is_aliased(), "the restored RAM is aliased");
        assert!(w.holds(0x10010) && w.holds(0x14000));
        assert_eq!(rt.window_stats().pages_outside, 0);
        assert_eq!(read(rt, 0x10010), 0x1234_5678);
        assert_eq!(rt.memory().ram_maps_file(), cfg!(target_os = "macos"), "shared on macOS, copied elsewhere");
    }
    // A store through a's window: a's Rust sees it, b and the image do not.
    // SAFETY: the window holds the page (checked above).
    unsafe { (a.flat_base().unwrap().add(0x10010) as *mut u32).write_volatile(7) };
    assert_eq!(a.memory().u32(0x10010).unwrap(), 7);
    assert_eq!((b.memory().u32(0x10010).unwrap(), read(&b, 0x10010)), (0x1234_5678, 0x1234_5678));
    // A store through b's Rust: b's window sees it, a does not.
    b.memory().set_u32(0x14020, 9).unwrap();
    assert_eq!(read(&b, 0x14020), 9);
    assert_eq!((a.memory().u32(0x14020).unwrap(), read(&a, 0x14020)), (0, 0));
    let image = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let at = physical(&a) + 0x10;
    assert_eq!(image[at..at + 4], 0x1234_5678u32.to_le_bytes(), "the image is never written");
    assert_eq!(physical(&a), physical(&b));
}
