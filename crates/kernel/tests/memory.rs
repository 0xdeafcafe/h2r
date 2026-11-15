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
    // There is no runtime unmap API: a valid decommit must stop, keeping RAM.
    h.stopped(199, &[base, size, MEM_DECOMMIT]);
    assert_eq!(h.get(0x12001000), 0x12345678);
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
fn committed_readonly_pages_enforce_permissions_and_release_stops() {
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
    h.stopped(199, &[DATA, DATA + 4, MEM_RELEASE]);
    assert_eq!(h.get(base), 0);
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
    h.stopped(171, &[p]);
    assert_eq!(h.get(p), 0x87654321);
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
    h.stopped(199, &[DATA, DATA + 4, MEM_DECOMMIT]);
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
