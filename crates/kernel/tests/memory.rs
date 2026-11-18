mod common;
use common::*;
use kernel::{memory::*, status};
use runtime::{
    Event,
    guest::{Access, Permissions},
};

#[test]
fn pool_exports_track_tags_alignment_size_lifetime_and_reuse() {
    let mut h = Harness::new();
    let p = h.success(15, &[31, 0x48414c4f]);
    assert_ne!(p, 0);
    assert_eq!(p % 16, 0);
    assert_eq!(h.success(23, &[p]), 32);
    h.bytes(p, &[1, 2, 3, 4]);
    assert_eq!(h.success(17, &[p]), 0xfeedface);
    h.stopped(17, &[p]);
    h.stopped(23, &[p]);
    let reused = h.success(14, &[16]);
    assert_eq!(reused, p);
    assert_eq!(h.success(23, &[reused]), 16);
    assert_eq!(h.success(14, &[u32::MAX]), 0);
    h.success(17, &[reused]);
    let p = h.success(14, &[0]);
    assert_ne!(p, 0);
    h.success(17, &[p]);
}

#[test]
fn virtual_reserve_commit_query_rounding_zero_fill_and_conflicts() {
    let mut h = Harness::new();
    let base = DATA;
    let size = DATA + 4;
    let info = DATA + 0x40;
    h.set(base, 0x12001234);
    h.set(size, 0x1000);
    assert_eq!(
        h.success(184, &[base, 0, size, MEM_RESERVE, PAGE_READWRITE]),
        status::SUCCESS
    );
    assert_eq!(h.get(base), 0x12000000);
    assert_eq!(h.get(size), 0x3000);
    assert!(h.runtime.memory().u32(0x12000000).is_err());
    assert_eq!(h.success(217, &[0x12000000, info]), status::SUCCESS);
    assert_eq!(h.get(info + 12), 0x3000);
    assert_eq!(h.get(info + 16), MEM_RESERVE);
    h.set(base, 0x12001001);
    h.set(size, 0xfff);
    assert_eq!(
        h.success(184, &[base, 0, size, MEM_COMMIT, PAGE_READWRITE]),
        status::SUCCESS
    );
    assert_eq!(h.get(base), 0x12001000);
    assert_eq!(h.get(size), 0x1000);
    assert_eq!(h.get(0x12001000), 0);
    h.set(0x12001000, 0x12345678);
    // Recommit preserves contents and original protection.
    assert_eq!(
        h.success(184, &[base, 0, size, MEM_COMMIT, PAGE_READONLY]),
        status::SUCCESS
    );
    assert_eq!(h.get(0x12001000), 0x12345678);
    assert_eq!(h.success(179, &[0x12001004]), PAGE_READWRITE);
    assert_eq!(h.success(180, &[0x12001004]), 0x3000);
    assert_eq!(h.success(217, &[0x12001004, info]), status::SUCCESS);
    assert_eq!(h.get(info), 0x12001000);
    assert_eq!(h.get(info + 12), 0x1000);
    assert_eq!(h.get(info + 16), MEM_COMMIT);
    assert_eq!(h.get(info + 20), PAGE_READWRITE);
    h.set(base, 0x12000000);
    h.set(size, 0x1000);
    assert_eq!(
        h.success(184, &[base, 0, size, MEM_RESERVE, PAGE_READWRITE]),
        status::CONFLICTING_ADDRESSES
    );
    // Decommit unmaps the committed page; the reservation remains.
    h.set(base, 0x12001000);
    assert_eq!(h.success(199, &[base, size, MEM_DECOMMIT]), status::SUCCESS);
    assert!(h.runtime.memory().u32(0x12001000).is_err());
    assert_eq!(h.success(217, &[0x12001000, info]), status::SUCCESS);
    assert_eq!(h.get(info + 16), MEM_RESERVE);
}

#[test]
fn reserved_only_release_is_real_and_query_reports_free_space() {
    let mut h = Harness::new();
    h.set(DATA, 0);
    h.set(DATA + 4, 0x2000);
    assert_eq!(
        h.success(184, &[DATA, 0, DATA + 4, MEM_RESERVE, PAGE_READWRITE]),
        0
    );
    let address = h.get(DATA);
    h.set(DATA + 4, 0);
    assert_eq!(h.success(199, &[DATA, DATA + 4, MEM_RELEASE]), 0);
    assert_eq!(h.success(217, &[address, DATA + 0x100]), 0);
    assert_eq!(h.get(DATA + 0x110), MEM_FREE);
    assert_eq!(
        h.success(199, &[DATA, DATA + 4, MEM_RELEASE]),
        status::MEMORY_NOT_ALLOCATED
    );
    assert_eq!(
        h.success(217, &[0x90000000, DATA + 0x100]),
        status::INVALID_PARAMETER
    );
}

#[test]
fn virtual_failures_do_not_allocate_or_modify_output_pointers() {
    let mut h = Harness::new();
    for (size, zero, flags, expected) in [
        (0, 0, MEM_COMMIT, status::INVALID_PARAMETER),
        (1, 22, MEM_COMMIT, status::INVALID_PARAMETER),
        (1, 0, 0, status::INVALID_PARAMETER),
        (u32::MAX, 0, MEM_COMMIT, status::NO_MEMORY),
    ] {
        h.set(DATA, 0);
        h.set(DATA + 4, size);
        let before = h.runtime.memory().mappings().to_vec();
        assert_eq!(
            h.success(184, &[DATA, zero, DATA + 4, flags, PAGE_READWRITE]),
            expected
        );
        assert_eq!(h.get(DATA), 0);
        assert_eq!(h.get(DATA + 4), size);
        assert_eq!(h.runtime.memory().mappings(), before);
    }
    h.set(DATA, 0);
    h.set(DATA + 4, 4096);
    h.stopped(184, &[DATA, 0, DATA + 4, MEM_COMMIT, 0x104]);
    let before = h.runtime.memory().mappings().to_vec();
    let t = h.call(184, &[0, 0, DATA + 4, MEM_COMMIT, PAGE_READWRITE]);
    assert!(matches!(
        h.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
    assert_eq!(h.runtime.memory().mappings(), before);
}

#[test]
fn committed_readonly_pages_enforce_permissions_and_release_unmaps_them() {
    let mut h = Harness::new();
    h.set(DATA, 0);
    h.set(DATA + 4, 1);
    assert_eq!(
        h.success(
            184,
            &[DATA, 0, DATA + 4, MEM_RESERVE | MEM_COMMIT, PAGE_READONLY]
        ),
        0
    );
    let base = h.get(DATA);
    assert_eq!(h.get(DATA + 4), 4096);
    assert_eq!(h.get(base), 0);
    assert!(h.runtime.memory().set_u32(base, 99).is_err());
    h.set(DATA + 4, 0);
    assert_eq!(h.success(199, &[DATA, DATA + 4, MEM_RELEASE]), 0);
    assert_eq!([h.get(DATA), h.get(DATA + 4)], [base, 4096]);
    assert!(h.runtime.memory().u32(base).is_err());
}

#[test]
fn contiguous_constraints_physical_translation_and_mmio_policy() {
    let mut h = Harness::new();
    let p = h.success(
        166,
        &[5000, 0x01000000, 0x0100ffff, 0x10000, PAGE_READWRITE],
    );
    assert_eq!(p, 0x81000000);
    assert_eq!(h.success(173, &[p + 123]), 0x01000000 + 123);
    assert_eq!(h.success(179, &[p + 123]), PAGE_READWRITE);
    assert_eq!(h.success(180, &[p + 123]), 8192);
    h.set(p, 0x87654321);
    // An explicit second VA demonstrates shared physical identity.
    h.runtime
        .memory()
        .map(0x20000000, 4096, 0x01000000, Permissions::RW)
        .unwrap();
    assert_eq!(h.get(0x20000000), 0x87654321);
    // Freeing unmaps p; the explicit alias still sees the bytes.
    h.success(171, &[p]);
    assert!(h.runtime.memory().u32(p).is_err());
    assert_eq!(h.get(0x20000000), 0x87654321);
    assert_eq!(
        h.success(166, &[4096, 0x01000000, 0x01000fff, 4096, PAGE_READWRITE]),
        0
    );
    assert_eq!(h.success(166, &[4096, 0, u32::MAX, 3, PAGE_READWRITE]), 0);
    assert_eq!(h.success(165, &[u32::MAX]), 0);
    assert_ne!(h.success(165, &[1]), 0);
    assert_ne!(h.success(166, &[4096, 0, u32::MAX, 4096, 0x404]), 0);
    h.stopped(177, &[0xfd000000, 4096, PAGE_READWRITE]);
    assert!(
        h.runtime
            .memory()
            .check(0xfd000000, 4, Access::Read, 1)
            .is_err()
    );
    h.stopped(173, &[0xdead0000]);
    h.stopped(179, &[0xdead0000]);
    h.stopped(180, &[0xdead0000]);
}

#[test]
fn virtual_commits_use_scattered_physical_pages_and_preflight_mapping_conflicts() {
    let mut h = Harness::new();
    h.runtime
        .memory()
        .map(0xc0000000, 0x01000000, 0, Permissions::RW)
        .unwrap();
    h.runtime
        .memory()
        .map(0xc1000000, 4096, 0x01001000, Permissions::RW)
        .unwrap();
    h.runtime
        .memory()
        .map(
            0xc2000000,
            (runtime::guest::RAM_SIZE as u32) - 0x01003000,
            0x01003000,
            Permissions::RW,
        )
        .unwrap();
    assert_eq!(h.success(165, &[8192]), 0);
    h.set(DATA, 0);
    h.set(DATA + 4, 8192);
    assert_eq!(
        h.success(
            184,
            &[DATA, 0, DATA + 4, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE]
        ),
        0
    );
    let base = h.get(DATA);
    assert_eq!(h.success(173, &[base]), 0x01000000);
    assert_eq!(h.success(173, &[base + 4096]), 0x01002000);
    assert_eq!(h.get(base), 0);
    assert_eq!(h.get(base + 4096), 0);

    let mut h = Harness::new();
    h.set(DATA, 0x12000000);
    h.set(DATA + 4, 4096);
    assert_eq!(
        h.success(184, &[DATA, 0, DATA + 4, MEM_RESERVE, PAGE_READWRITE]),
        0
    );
    h.runtime
        .memory()
        .map(0x12000000, 4096, 0x03000000, Permissions::RW)
        .unwrap();
    let before = h.runtime.memory().mappings().to_vec();
    assert_eq!(
        h.success(184, &[DATA, 0, DATA + 4, MEM_COMMIT, PAGE_READWRITE]),
        status::CONFLICTING_ADDRESSES
    );
    assert_eq!(h.runtime.memory().mappings(), before);
}

#[test]
fn zero_bits_can_select_low_addresses_and_unclassified_mappings_do_not_fake_image_metadata() {
    let mut h = Harness::new();
    h.set(DATA, 0);
    h.set(DATA + 4, 4096);
    assert_eq!(
        h.success(
            184,
            &[DATA, 8, DATA + 4, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE]
        ),
        0
    );
    let base = h.get(DATA);
    assert!((0x10000..0x01000000).contains(&base));
    assert_eq!(h.success(217, &[base, DATA + 0x100]), 0);
    assert_eq!(h.get(DATA + 0x118), MEM_PRIVATE);
    h.stopped(217, &[CODE, DATA + 0x100]);
    h.set(DATA, 0);
    h.set(DATA + 4, 8192);
    assert_eq!(
        h.success(
            184,
            &[DATA, 0, DATA + 4, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE]
        ),
        0
    );
    let base = h.get(DATA);
    h.set(DATA, base + 4096);
    h.set(DATA + 4, 4096);
    assert_eq!(h.success(199, &[DATA, DATA + 4, MEM_DECOMMIT]), 0);
    assert_eq!(h.get(base), 0);
    assert!(h.runtime.memory().u32(base + 4096).is_err());
}

#[test]
fn physical_translation_of_the_last_guest_byte_does_not_overflow() {
    let mut h = Harness::new();
    h.runtime
        .memory()
        .map(0xffff_f000, 4096, 0x100000, Permissions::RW)
        .unwrap();
    assert_eq!(h.success(173, &[0xffff_ffff]), 0x100fff);
    assert_eq!(h.success(173, &[0xffff_f000]), 0x100000);
}

#[test]
fn hle_reserved_range_is_never_allocated_to_the_guest() {
    assert_eq!(HLE_RESERVED, 0xa000_0000..0xa100_0000);
    let mut h = Harness::new();
    for (base, size) in [
        (HLE_RESERVED_START, 0x10000),
        (HLE_RESERVED_END - 0x1000, 0x1000),
        (HLE_RESERVED_START - 0x10000, 0x20000),
    ] {
        for flags in [MEM_RESERVE, MEM_RESERVE | MEM_COMMIT] {
            h.set(DATA, base);
            h.set(DATA + 4, size);
            assert_eq!(
                h.success(184, &[DATA, 0, DATA + 4, flags, 4]),
                status::CONFLICTING_ADDRESSES
            );
            assert_eq!(h.get(DATA), base);
        }
    }
    // Automatic placement and contiguous memory land elsewhere.
    h.set(DATA, 0);
    h.set(DATA + 4, 0x10000);
    assert_eq!(h.success(184, &[DATA, 0, DATA + 4, MEM_RESERVE, 4]), 0);
    assert!(!HLE_RESERVED.contains(&h.get(DATA)));
    let contiguous = h.success(165, &[0x10000]);
    assert_ne!(contiguous, 0);
    assert!(!HLE_RESERVED.contains(&contiguous));
    for _ in 0..4 {
        let pool = h.success(14, &[0x8000]);
        assert!(!HLE_RESERVED.contains(&pool));
    }
}

fn readable(h: &Harness, va: u32) -> bool {
    h.runtime.memory().check(va, 4, Access::Read, 1).is_ok()
}

/// The retail caller at 0x335653: MmCreateKernelStack(0x6000, FALSE) returns
/// the top, and the stack is later deleted as (top, top - 0x6000).
#[test]
fn kernel_stacks_are_kernel_owned_guarded_zeroed_and_reused() {
    let mut h = Harness::new();
    let top = h.success(169, &[0x6000, 0]);
    let bottom = top - 0x6000;
    assert_eq!(top % 4096, 0);
    assert!((0xa100_0000..0xb000_0000).contains(&bottom), "{bottom:#x}");
    assert!(!HLE_RESERVED.contains(&bottom) && !HLE_RESERVED.contains(&(top - 1)));
    assert!(readable(&h, bottom) && readable(&h, top - 4));
    assert!(!readable(&h, bottom - 4), "guard page below the stack");
    assert_eq!(h.get(top - 4), 0);
    h.set(top - 4, 0x1234_5678);
    // A second live stack is distinct, and not adjacent to the first.
    let other = h.success(169, &[0x5001, 0]);
    assert_ne!(other, top);
    assert!(!readable(&h, other - 0x6000 - 4));
    h.success(170, &[top, bottom]);
    // Double delete and foreign ranges stop.
    h.stopped(170, &[top, bottom]);
    h.stopped(170, &[0x7000_0000, 0x6fff_a000]);
    // A deleted stack of the same size is handed out again, zeroed.
    assert_eq!(h.success(169, &[0x6000, 0]), top);
    assert_eq!(h.get(top - 4), 0);
    // Mismatched bounds are not that stack.
    h.stopped(170, &[top, bottom + 0x1000]);
    h.stopped(169, &[0x6000, 1]);
    h.stopped(169, &[0, 0]);
}

#[test]
fn mem_top_down_places_new_reservations_highest_first() {
    let mut h = Harness::new();
    let reserve = |h: &mut Harness, base: u32, zero_bits: u32, flags: u32| {
        h.set(DATA, base);
        h.set(DATA + 4, 0x10000);
        let s = h.success(184, &[DATA, zero_bits, DATA + 4, flags, PAGE_READWRITE]);
        (s, h.get(DATA))
    };
    let td = MEM_RESERVE | MEM_TOP_DOWN;
    assert_eq!(reserve(&mut h, 0, 0, td), (0, 0x7ffe_0000));
    assert_eq!(reserve(&mut h, 0, 0, td | MEM_COMMIT), (0, 0x7ffd_0000));
    assert_eq!(h.get(0x7ffd_0000), 0);
    // Bottom-up placement is unaffected.
    let (s, low) = reserve(&mut h, 0, 0, MEM_RESERVE);
    assert_eq!(s, 0);
    assert!(low < 0x7000_0000, "{low:#x}");
    // ZeroBits bounds the top.
    assert_eq!(reserve(&mut h, 0, 4, td), (0, 0x0fff_0000));
    // A fixed base ignores the flag.
    assert_eq!(reserve(&mut h, 0x3000_0000, 0, td), (0, 0x3000_0000));
    // TOP_DOWN alone, or with unknown flags, is invalid.
    assert_eq!(
        reserve(&mut h, 0, 0, MEM_TOP_DOWN).0,
        status::INVALID_PARAMETER
    );
    assert_eq!(
        reserve(&mut h, 0, 0, td | 0x40_0000).0,
        status::INVALID_PARAMETER
    );
}

/// A Runtime physical reservation (Halo 2's is `LoadOptions::halo2()`'s
/// 0x61000..0x3145000 with alias 0x80061000) is never used by kernel
/// placements, and only contiguous requests bounded by it map onto it.
#[test]
fn physical_reservations_are_only_used_by_requests_inside_them() {
    const WINDOW: std::ops::Range<u32> = 0x10_0000..0x20_0000;
    let mut h = Harness::with_guest(kernel::Config::default(), |g| {
        g.reserve_physical(
            WINDOW.start,
            WINDOW.end - WINDOW.start,
            &[0x8010_0000],
            "test",
        )
        .unwrap();
    });
    let physical = |h: &mut Harness, va: u32| h.success(173, &[va]);
    for _ in 0..8 {
        let pool = h.success(14, &[0x8000]);
        assert!(!WINDOW.contains(&physical(&mut h, pool)));
    }
    let top = h.success(169, &[0x6000, 0]);
    for page in (top - 0x6000..top).step_by(4096) {
        assert!(!WINDOW.contains(&physical(&mut h, page)));
    }
    // Bounds straddling the window do not open it.
    let straddle = h.success(166, &[0x1000, 0x10_0000, 0x2f_ffff, 0, 4]);
    assert_ne!(straddle, 0);
    assert!(!WINDOW.contains(&physical(&mut h, straddle)));
    // 4 MiB of committed pages skip the whole window.
    h.set(DATA, 0);
    h.set(DATA + 4, 0x40_0000);
    assert_eq!(
        h.success(184, &[DATA, 0, DATA + 4, MEM_RESERVE | MEM_COMMIT, 4]),
        0
    );
    let base = h.get(DATA);
    for page in (base..base + 0x40_0000).step_by(4096) {
        assert!(!WINDOW.contains(&physical(&mut h, page)), "{page:#x}");
    }
    let anywhere = h.success(165, &[0x10000]);
    assert!(!WINDOW.contains(&physical(&mut h, anywhere)));
    // Requests bounded by the reservation map onto it at 0x80000000 + phys.
    let arena = h.success(166, &[0x4000, 0x10_0000, 0x1f_ffff, 0, 4]);
    assert_eq!(arena, 0x8010_0000);
    assert_eq!(physical(&mut h, arena + 0x10), 0x10_0010);
    let next = h.success(166, &[0x1000, 0x10_0000, 0x1f_ffff, 0x2000, 4]);
    assert_eq!(next, 0x8010_4000);
    h.set(arena, 0x1234_5678);
    // The whole reservation is no longer free; too big is 0.
    assert_eq!(h.success(166, &[0x10_0000, 0x10_0000, 0x1f_ffff, 0, 4]), 0);
    // Freed reserved pages stay reserved, keep their bytes, and come back.
    h.success(171, &[arena]);
    assert!(h.runtime.memory().u32(arena).is_err());
    assert_eq!(h.success(166, &[0x4000, 0x10_0000, 0x1f_ffff, 0, 4]), arena);
    assert_eq!(h.get(arena), 0x1234_5678);
}

#[test]
fn a_reservation_without_the_contiguous_alias_stops() {
    let mut h = Harness::with_guest(kernel::Config::default(), |g| {
        g.reserve_physical(0x10_0000, 0x10_0000, &[0x9000_0000], "test")
            .unwrap();
    });
    h.stopped(166, &[0x1000, 0x10_0000, 0x1f_ffff, 0, 4]);
}

#[test]
fn freeing_contiguous_memory_unmaps_it_and_frees_its_pages() {
    let mut h = Harness::new();
    let p = h.success(166, &[0x2000, 0x0100_0000, 0x0100_ffff, 0, 4]);
    assert_eq!(p, 0x8100_0000);
    h.success(182, &[p, 0x1000, 0x404]);
    h.success(178, &[p, 0x2000, 1]);
    assert_eq!(h.kernel.persistent_pages(), [p, p + 0x1000]);
    h.success(171, &[p]);
    assert!(h.kernel.persistent_pages().is_empty());
    assert!(h.runtime.memory().u32(p).is_err());
    h.stopped(171, &[p]);
    h.stopped(179, &[p]);
    // The physical pages are free for the next allocation.
    assert_eq!(h.success(166, &[0x2000, 0x0100_0000, 0x0100_ffff, 0, 4]), p);
    assert_eq!(h.success(179, &[p]), PAGE_READWRITE);
    h.stopped(171, &[p + 0x1000]);
}

#[test]
fn decommit_and_release_unmap_committed_pages() {
    let mut h = Harness::new();
    let (base_ptr, size_ptr) = (DATA, DATA + 4);
    h.set(base_ptr, 0);
    h.set(size_ptr, 0x4000);
    assert_eq!(
        h.success(184, &[base_ptr, 0, size_ptr, MEM_RESERVE | MEM_COMMIT, 4]),
        0
    );
    let base = h.get(base_ptr);
    let used = h.success(173, &[base + 0x1000]);
    // Decommit one unaligned page's worth: whole pages, rounded out.
    h.set(base_ptr, base + 0x1010);
    h.set(size_ptr, 0x10);
    assert_eq!(h.success(199, &[base_ptr, size_ptr, MEM_DECOMMIT]), 0);
    assert_eq!([h.get(base_ptr), h.get(size_ptr)], [base + 0x1000, 0x1000]);
    assert!(h.runtime.memory().u32(base + 0x1000).is_err());
    assert_eq!(h.get(base), 0);
    assert_eq!(h.success(217, &[base + 0x1000, DATA + 0x40]), 0);
    assert_eq!(h.get(DATA + 0x40 + 16), MEM_RESERVE);
    // Its physical page is free again.
    assert!(h.runtime.memory().physical_free(used, 0x1000));
    // Recommit gives zeroed memory.
    h.set(base_ptr, base + 0x1000);
    h.set(size_ptr, 0x1000);
    assert_eq!(h.success(184, &[base_ptr, 0, size_ptr, MEM_COMMIT, 4]), 0);
    assert_eq!(h.get(base + 0x1000), 0);
    // Decommit with size 0 runs to the end of the allocation.
    h.set(base_ptr, base + 0x2000);
    h.set(size_ptr, 0);
    assert_eq!(h.success(199, &[base_ptr, size_ptr, MEM_DECOMMIT]), 0);
    assert_eq!(h.get(size_ptr), 0x2000);
    assert!(h.runtime.memory().u32(base + 0x3000).is_err());
    // Past the allocation stops; release needs size 0 and the base.
    h.set(base_ptr, base);
    h.set(size_ptr, 0x5000);
    h.stopped(199, &[base_ptr, size_ptr, MEM_DECOMMIT]);
    assert_eq!(
        h.success(199, &[base_ptr, size_ptr, MEM_RELEASE]),
        status::INVALID_PARAMETER
    );
    h.set(size_ptr, 0);
    assert_eq!(h.success(199, &[base_ptr, size_ptr, MEM_RELEASE]), 0);
    assert_eq!([h.get(base_ptr), h.get(size_ptr)], [base, 0x4000]);
    assert!(h.runtime.memory().u32(base).is_err());
    assert_eq!(
        h.success(199, &[base_ptr, size_ptr, MEM_RELEASE]),
        status::MEMORY_NOT_ALLOCATED
    );
}

/// Halo's observed calls keep RW and only add write-combining (0x404) or
/// restate PAGE_READWRITE, on contiguous and committed memory.
#[test]
fn set_address_protect_changes_exactly_the_requested_pages() {
    let mut h = Harness::new();
    let p = h.success(165, &[0x3000]);
    h.set(p + 0x1000, 7);
    h.success(182, &[p + 0x1000, 0x1000, 0x404]);
    assert_eq!(h.success(179, &[p + 0x1000]), 0x404);
    assert_eq!(h.success(179, &[p]), PAGE_READWRITE);
    assert_eq!(h.get(p + 0x1000), 7, "contents survive");
    // Size 0 is a no-op.
    h.success(182, &[p, 0, PAGE_READONLY]);
    assert_eq!(h.success(179, &[p]), PAGE_READWRITE);
    // One page in the middle of a single mapping: only that page changes.
    let writable = |h: &Harness, va: u32| h.runtime.memory().check(va, 4, Access::Write, 1).is_ok();
    h.success(182, &[p + 0x1000, 0x1000, PAGE_READONLY]);
    assert_eq!(h.success(179, &[p + 0x1000]), PAGE_READONLY);
    assert!(writable(&h, p) && !writable(&h, p + 0x1000) && writable(&h, p + 0x2000));
    assert_eq!(h.get(p + 0x1000), 7, "still readable");
    // PAGE_NOACCESS faults every access.
    h.success(182, &[p + 0x2000, 0x1000, PAGE_NOACCESS]);
    assert!(h.runtime.memory().u32(p + 0x2000).is_err());
    assert_eq!(h.success(179, &[p + 0x2000]), PAGE_NOACCESS);
    // An alias of the same physical pages keeps its own protection.
    let phys = h.success(173, &[p + 0x1000]);
    h.runtime
        .memory()
        .map(0x2000_0000, 0x1000, phys, Permissions::RW)
        .unwrap();
    h.set(0x2000_0000, 9);
    assert_eq!(h.get(p + 0x1000), 9);
    // The whole allocation, across the split pieces.
    h.success(182, &[p, 0x3000, PAGE_READWRITE]);
    assert!(writable(&h, p) && writable(&h, p + 0x1000) && writable(&h, p + 0x2000));
    h.success(182, &[p, 0x3000, PAGE_READONLY]);
    assert_eq!(h.success(179, &[p + 0x2000]), PAGE_READONLY);
    assert!(!writable(&h, p));
    // Committed virtual memory, unaligned base and length.
    h.set(DATA, 0);
    h.set(DATA + 4, 0x2000);
    assert_eq!(
        h.success(184, &[DATA, 0, DATA + 4, MEM_RESERVE | MEM_COMMIT, 4]),
        0
    );
    let v = h.get(DATA);
    h.success(182, &[v + 0x10, 0x1ff0, 0x404]);
    assert_eq!(h.success(179, &[v + 0x1800]), 0x404);
    // Reserved-only and unallocated pages stop.
    h.set(DATA, 0);
    h.set(DATA + 4, 0x1000);
    assert_eq!(h.success(184, &[DATA, 0, DATA + 4, MEM_RESERVE, 4]), 0);
    let reserved = h.get(DATA);
    h.stopped(182, &[reserved, 0x1000, 4]);
    h.stopped(182, &[0x5000_0000, 0x1000, 4]);
    h.stopped(182, &[p, 0x1000, 0x1234]);
}

/// Pages Runtime guards as lifted code (`Guest::guard_code`) cannot lose
/// Execute or be unmapped: 182, 171 and 199 stop and change nothing.
#[test]
fn lifted_code_pages_refuse_protection_loss_and_frees() {
    let mut h = Harness::new();
    let p = h.success(
        166,
        &[0x2000, 0x0100_0000, 0x0100_ffff, 0, PAGE_EXECUTE_READWRITE],
    );
    assert_ne!(p, 0);
    h.runtime.memory().guard_code(p + 0x1000, 0x10).unwrap();
    h.stopped(182, &[p + 0x1000, 0x1000, PAGE_READWRITE]);
    assert_eq!(h.success(179, &[p + 0x1000]), PAGE_EXECUTE_READWRITE);
    // Keeping Execute is allowed.
    h.success(182, &[p + 0x1000, 0x1000, PAGE_EXECUTE_READ]);
    h.stopped(171, &[p]);
    assert!(h.runtime.memory().u32(p).is_ok());
    assert_eq!(h.success(180, &[p]), 0x2000);
    // Committed virtual memory: decommit and release stop before unmapping.
    h.set(DATA, 0);
    h.set(DATA + 4, 0x2000);
    assert_eq!(
        h.success(
            184,
            &[
                DATA,
                0,
                DATA + 4,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_EXECUTE_READWRITE
            ]
        ),
        0
    );
    let v = h.get(DATA);
    h.runtime.memory().guard_code(v + 0x1000, 0x1000).unwrap();
    h.set(DATA + 4, 0);
    h.stopped(199, &[DATA, DATA + 4, MEM_DECOMMIT]);
    h.stopped(199, &[DATA, DATA + 4, MEM_RELEASE]);
    assert!(h.runtime.memory().u32(v).is_ok() && h.runtime.memory().u32(v + 0x1000).is_ok());
    // The unguarded first page alone can still be decommitted.
    h.set(DATA + 4, 0x1000);
    assert_eq!(h.success(199, &[DATA, DATA + 4, MEM_DECOMMIT]), 0);
    assert!(h.runtime.memory().u32(v).is_err());
}

fn last_stop(h: &Harness) -> &'static str {
    h.kernel
        .stop_reasons()
        .last()
        .expect("a stop was recorded")
        .1
}

/// A reservation's alias window can only be filled by `map_reserved`, so a
/// virtual allocation there is refused up front rather than reaching
/// `Guest::map` (which returns `Reserved`).
#[test]
fn virtual_allocations_avoid_reservation_alias_windows() {
    let mut h = Harness::with_guest(kernel::Config::default(), |g| {
        g.reserve_physical(0x0300_0000, 0x1000, &[0x1200_0000], "test")
            .unwrap();
    });
    for flags in [MEM_RESERVE, MEM_RESERVE | MEM_COMMIT] {
        h.set(DATA, 0x1200_0000);
        h.set(DATA + 4, 0x1000);
        assert_eq!(
            h.success(184, &[DATA, 0, DATA + 4, flags, PAGE_READWRITE]),
            status::CONFLICTING_ADDRESSES
        );
        assert_eq!([h.get(DATA), h.get(DATA + 4)], [0x1200_0000, 0x1000]);
    }
    // A reservation made after the kernel reserved (but did not commit)
    // the range: committing there is refused, and nothing is committed.
    h.set(DATA, 0x1300_0000);
    h.set(DATA + 4, 0x2000);
    assert_eq!(
        h.success(184, &[DATA, 0, DATA + 4, MEM_RESERVE, PAGE_READWRITE]),
        0
    );
    h.runtime
        .memory()
        .reserve_physical(0x0310_0000, 0x1000, &[0x1300_1000], "late")
        .unwrap();
    h.set(DATA + 4, 0x2000);
    assert_eq!(
        h.success(184, &[DATA, 0, DATA + 4, MEM_COMMIT, PAGE_READWRITE]),
        status::CONFLICTING_ADDRESSES
    );
    assert!(!readable(&h, 0x1300_0000), "no partial commit");
    assert_eq!(h.success(217, &[0x1300_0000, DATA + 0x40]), 0);
    assert_eq!(h.get(DATA + 0x40 + 16), MEM_RESERVE);
    // The page outside the window still commits.
    h.set(DATA, 0x1300_0000);
    h.set(DATA + 4, 0x1000);
    assert_eq!(
        h.success(184, &[DATA, 0, DATA + 4, MEM_COMMIT, PAGE_READWRITE]),
        0
    );
    assert!(readable(&h, 0x1300_0000));
    assert!(h.kernel.stop_reasons().is_empty());
}

/// MmDeleteKernelStack over a range Runtime refuses to unmap (an HLE claim
/// on its last page) stops, and the stack stays live, mapped and guarded.
#[test]
fn kernel_stack_delete_inside_a_claim_stops_and_keeps_the_stack() {
    let mut h = Harness::new();
    let top = h.success(169, &[0x2000, 0]);
    let bottom = top - 0x2000;
    h.runtime
        .memory()
        .claim_virtual(top - 0x1000, 0x1000, "test")
        .unwrap();
    h.stopped(170, &[top, bottom]);
    assert_eq!(
        last_stop(&h),
        "deleting a kernel stack inside an HLE-owned claim"
    );
    assert!(readable(&h, bottom) && readable(&h, top - 4));
    // Still the same live stack: deleting it again hits the claim again,
    // not "not created".
    h.stopped(170, &[top, bottom]);
    assert_eq!(
        last_stop(&h),
        "deleting a kernel stack inside an HLE-owned claim"
    );
    // Its guard is still tracked.
    for _ in 0..8 {
        let pool = h.success(14, &[0x1000]);
        assert!(!(bottom - 0x1000..top).contains(&pool), "{pool:#x}");
    }
}

/// NtFreeVirtualMemory is all-or-nothing: with the second of two committed
/// pages claimed by HLE, MEM_RELEASE and MEM_DECOMMIT both stop before the
/// first page is unmapped or forgotten.
#[test]
fn virtual_free_refusal_changes_nothing() {
    let mut h = Harness::new();
    h.set(DATA, 0);
    h.set(DATA + 4, 0x2000);
    assert_eq!(
        h.success(184, &[DATA, 0, DATA + 4, MEM_RESERVE | MEM_COMMIT, 4]),
        0
    );
    let base = h.get(DATA);
    let first = h.success(173, &[base]);
    h.set(base, 0x1234_5678);
    h.runtime
        .memory()
        .claim_virtual(base + 0x1000, 0x1000, "test")
        .unwrap();
    for (kind, size) in [(MEM_RELEASE, 0), (MEM_DECOMMIT, 0x2000)] {
        h.set(DATA, base);
        h.set(DATA + 4, size);
        h.stopped(199, &[DATA, DATA + 4, kind]);
        assert_eq!(
            last_stop(&h),
            "freeing virtual memory inside an HLE-owned claim"
        );
        assert_eq!([h.get(DATA), h.get(DATA + 4)], [base, size]);
        assert_eq!(h.get(base), 0x1234_5678, "first page still mapped");
        assert!(readable(&h, base + 0x1000));
        assert!(!h.runtime.memory().physical_free(first, 0x1000));
        for page in [base, base + 0x1000] {
            assert_eq!(h.success(217, &[page, DATA + 0x40]), 0);
            assert_eq!(h.get(DATA + 0x40 + 16), MEM_COMMIT, "{page:#x}");
        }
    }
    // The unclaimed page alone still decommits.
    h.set(DATA, base);
    h.set(DATA + 4, 0x1000);
    assert_eq!(h.success(199, &[DATA, DATA + 4, MEM_DECOMMIT]), 0);
    assert!(!readable(&h, base));
    assert!(h.runtime.memory().physical_free(first, 0x1000));
}

/// A live kernel stack's guard page is never handed to a later kernel
/// allocation, even though it is unmapped.
#[test]
fn kernel_stack_guard_is_not_reused_while_the_stack_lives() {
    let mut h = Harness::new();
    let top = h.success(169, &[0x2000, 0]);
    let guard = top - 0x2000 - 0x1000;
    assert!(!readable(&h, guard));
    // Page-sized pool blocks fill the internal range lowest first; keep
    // going until one lands above the stack, so every gap below it is used.
    let mut above = false;
    for _ in 0..64 {
        let pool = h.success(14, &[0x1000]);
        assert_ne!(pool, 0);
        assert!(
            !(guard..top).contains(&pool),
            "{pool:#x} inside the stack span"
        );
        if pool > top {
            above = true;
            break;
        }
    }
    assert!(above, "pool allocations never passed the stack");
    assert!(!readable(&h, guard), "guard page still unmapped");
    // Deleting the stack frees its guard along with it.
    h.success(170, &[top, top - 0x2000]);
    let pool = h.success(14, &[0x1000]);
    assert!(pool < top, "{pool:#x}");
}

#[test]
fn query_statistics_checks_length_and_counts_what_the_allocators_hold() {
    let mut h = Harness::new();
    let out = DATA + 0x100;
    let stats = |h: &Harness| -> Vec<u32> { (0..9).map(|i| h.get(out + 4 * i)).collect() };
    // A wrong Length is refused and nothing else is written.
    h.set(out, 0x20);
    h.set(out + 4, 0xdead);
    assert_eq!(h.success(181, &[out]), status::INVALID_PARAMETER);
    assert_eq!((h.get(out), h.get(out + 4)), (0x20, 0xdead));

    h.set(out, MM_STATISTICS_SIZE);
    assert_eq!(h.success(181, &[out]), status::SUCCESS);
    let before = stats(&h);
    assert_eq!(before[0], 0x24);
    assert_eq!(before[1], 64 * 1024 * 1024 / 4096, "64 MB of pages");
    assert!(before[2] > 0 && before[2] < before[1], "{before:x?}");

    // 16 KiB of contiguous memory: four pages fewer available.
    let c = h.success(165, &[0x4000]);
    assert_ne!(c, 0);
    // Reserve 64 KiB, commit 8 KiB of it.
    let (base, size) = (DATA, DATA + 4);
    h.set(base, 0);
    h.set(size, 0x1_0000);
    assert_eq!(
        h.success(184, &[base, 0, size, MEM_RESERVE, PAGE_READWRITE]),
        status::SUCCESS
    );
    let region = h.get(base);
    h.set(size, 0x2000);
    assert_eq!(
        h.success(184, &[base, 0, size, MEM_COMMIT, PAGE_READWRITE]),
        status::SUCCESS
    );
    assert_eq!(h.get(base), region);
    h.set(out, MM_STATISTICS_SIZE);
    assert_eq!(h.success(181, &[out]), status::SUCCESS);
    let after = stats(&h);
    assert_eq!(after[1], before[1]);
    assert_eq!(
        before[2] - after[2],
        4 + 2,
        "contiguous and committed pages"
    );
    assert_eq!(after[3] - before[3], 0x2000, "committed bytes");
    assert_eq!(after[4] - before[4], 0x1_0000, "reserved bytes");
}
