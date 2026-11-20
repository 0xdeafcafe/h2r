//! Range protection, unmapping, physical reservations and HLE claims.
mod common;
use common::*;
use iced_x86::{Decoder, DecoderOptions, Instruction, Mnemonic, OpKind, Register};
use runtime::{
    Event, Runner, TransferAbi, abi,
    guest::{Access, FaultReason, Guest, MapError, Mapping, PAGE_SIZE, Permissions, RAM_SIZE},
    load_xbe, load_xbe_with,
    loader::{HALO2_PHYSICAL_ARENA, LoadError, LoadOptions, ReservationRequest},
};

const P: u32 = PAGE_SIZE;
const ARENA: u32 = HALO2_PHYSICAL_ARENA.start;
const ARENA_LEN: u32 = HALO2_PHYSICAL_ARENA.end - HALO2_PHYSICAL_ARENA.start;
const ARENA_VA: u32 = 0x8000_0000 + ARENA;

fn m(virtual_start: u32, len: u32, physical_start: u32, permissions: Permissions) -> Mapping {
    Mapping {
        virtual_start,
        len,
        physical_start,
        permissions,
    }
}
fn reason<T: std::fmt::Debug>(r: Result<T, runtime::GuestFault>) -> FaultReason {
    r.unwrap_err().reason
}
/// Four RW pages at 0x10000 backed by physical 0x8000.
fn four_pages() -> Guest {
    let mut g = Guest::new();
    g.map(0x10000, 4 * P, 0x8000, Permissions::RW).unwrap();
    g
}

#[test]
fn protect_range_splits_at_both_edges_and_keeps_physical_offsets() {
    let mut g = four_pages();
    g.set_u32(0x11ffc, 0x1111_1111).unwrap();
    g.set_u32(0x12000, 0x2222_2222).unwrap();
    g.protect_range(0x11000, P, Permissions::READ).unwrap();
    assert_eq!(
        g.mappings(),
        &[
            m(0x10000, P, 0x8000, Permissions::RW),
            m(0x11000, P, 0x9000, Permissions::READ),
            m(0x12000, 2 * P, 0xa000, Permissions::RW),
        ]
    );
    // Bytes stay where they were; only the middle page refuses writes.
    assert_eq!(g.u32(0x11ffc).unwrap(), 0x1111_1111);
    assert_eq!(g.u32(0x12000).unwrap(), 0x2222_2222);
    g.set_u32(0x10ffc, 1).unwrap();
    assert_eq!(reason(g.set_u8(0x11000, 1)), FaultReason::Protection);
    assert_eq!(reason(g.set_u8(0x11fff, 1)), FaultReason::Protection);
    g.set_u8(0x12000, 1).unwrap();
    // A write straddling the lower edge changes nothing on either side.
    assert_eq!(
        reason(g.set_u32(0x10ffe, 0xdead_beef)),
        FaultReason::Protection
    );
    assert_eq!(g.u32(0x10ffc).unwrap(), 1);
    // A cross-piece read still works: pieces are adjacent and contiguous.
    assert_eq!(g.u64(0x11ffc).unwrap(), 0x2222_2201_1111_1111);
    // Protecting a piece that already has exact edges does not split again.
    g.protect_range(0x12000, 2 * P, Permissions::RX).unwrap();
    assert_eq!(g.mappings().len(), 3);
    assert!(g.check(0x13fff, 1, Access::Execute, 1).is_ok());
}

#[test]
fn protect_range_spans_several_mappings_and_none_denies_everything() {
    let mut g = Guest::new();
    g.map(0x10000, 2 * P, 0, Permissions::RWX).unwrap();
    g.map(0x12000, 2 * P, 0x20000, Permissions::RW).unwrap();
    g.protect_range(0x11000, 2 * P, Permissions::NONE).unwrap();
    assert_eq!(
        g.mappings(),
        &[
            m(0x10000, P, 0, Permissions::RWX),
            m(0x11000, P, 0x1000, Permissions::NONE),
            m(0x12000, P, 0x20000, Permissions::NONE),
            m(0x13000, P, 0x21000, Permissions::RW),
        ]
    );
    for access in [Access::Read, Access::Write, Access::Execute] {
        assert_eq!(
            g.check(0x11000, 1, access, 1).unwrap_err().reason,
            FaultReason::Protection
        );
        assert_eq!(
            g.check(0x12fff, 1, access, 1).unwrap_err().reason,
            FaultReason::Protection
        );
    }
    assert!(g.check(0x10fff, 1, Access::Execute, 1).is_ok());
    assert!(g.check(0x13000, 1, Access::Write, 1).is_ok());
    // Execute is removed when narrowing RWX to RW.
    g.protect_range(0x10000, P, Permissions::RW).unwrap();
    assert_eq!(
        g.check(0x10000, 1, Access::Execute, 1).unwrap_err().reason,
        FaultReason::Protection
    );
}

#[test]
fn range_operations_validate_everything_before_changing_anything() {
    let mut g = four_pages();
    g.map(0x20000, P, 0x40000, Permissions::RW).unwrap(); // hole at 0x14000..0x20000
    let claim = g.claim_virtual(0x30000, P, "hle").unwrap();
    g.map(0x30000, P, 0x50000, Permissions::RW).unwrap();
    let before = g.mappings().to_vec();
    let bad: [(u32, u32, MapError); 9] = [
        (0x10000, 0, MapError::InvalidRange),
        (0x10800, P, MapError::InvalidRange),
        (0x10000, 0x800, MapError::InvalidRange),
        (0xffff_f000, 2 * P, MapError::InvalidRange),
        (0x0f000, 2 * P, MapError::Unmapped), // hole before the first page
        (0x13000, 2 * P, MapError::Unmapped), // hole after the last page
        (0x11000, 0x10000, MapError::Unmapped), // hole in the middle
        (0x2f000, 2 * P, MapError::Claimed),  // straddles a claim edge
        (0x30000, P, MapError::Claimed),      // wholly claimed
    ];
    for (va, len, err) in bad {
        assert_eq!(
            g.protect_range(va, len, Permissions::READ),
            Err(err),
            "{va:#x}+{len:#x}"
        );
        assert_eq!(g.unmap(va, len), Err(err), "{va:#x}+{len:#x}");
        assert_eq!(
            g.mappings(),
            before.as_slice(),
            "{va:#x}+{len:#x} changed state"
        );
    }
    // A failed protect leaves every page writable.
    for page in (0x10000..0x14000).step_by(P as usize) {
        g.set_u8(page, 7).unwrap();
    }
    // Owned variants: the range must lie inside the owner's claim.
    assert_eq!(g.unmap_owned(claim, 0x2f000, 2 * P), Err(MapError::Claimed));
    assert_eq!(
        g.protect_range_owned(claim, 0x10000, P, Permissions::READ),
        Err(MapError::Claimed)
    );
    assert_eq!(g.mappings(), before.as_slice());
}

#[test]
fn protection_is_per_virtual_alias_and_bytes_stay_coherent() {
    let mut g = Guest::new();
    g.map(0x10000, 3 * P, 0, Permissions::RW).unwrap();
    g.map(0x8000_0000, 3 * P, 0, Permissions::RW).unwrap();
    g.protect_range(0x11000, P, Permissions::READ).unwrap();
    // The other alias keeps its own permissions and one unsplit mapping.
    assert_eq!(
        g.mappings()
            .iter()
            .filter(|m| m.virtual_start >= 0x8000_0000)
            .count(),
        1
    );
    g.set_u32(0x8000_1800, 0xfeed_f00d).unwrap();
    assert_eq!(g.u32(0x11800).unwrap(), 0xfeed_f00d);
    assert_eq!(reason(g.set_u32(0x11800, 0)), FaultReason::Protection);
    assert_eq!(g.u32(0x8000_1800).unwrap(), 0xfeed_f00d);
    // Writes through the low pieces still reach the high alias.
    g.set_u32(0x12000, 0xabcd).unwrap();
    assert_eq!(g.u32(0x8000_2000).unwrap(), 0xabcd);
    g.set_u32(0x10ffc, 0x1234).unwrap();
    assert_eq!(g.u32(0x8000_0ffc).unwrap(), 0x1234);
    let aliases = g.physical_aliases(0x1000, P);
    assert_eq!(
        aliases,
        vec![
            m(0x11000, P, 0x1000, Permissions::READ),
            m(0x8000_1000, P, 0x1000, Permissions::RW),
        ]
    );
    // Clipped to the physical query: a partial page of a larger window.
    assert_eq!(
        g.physical_aliases(0x2800, 0x1000),
        vec![
            m(0x12800, 0x800, 0x2800, Permissions::RW),
            m(0x8000_2800, 0x800, 0x2800, Permissions::RW),
        ]
    );
}

#[test]
fn unmap_splits_edges_faults_the_hole_and_keeps_alias_bytes() {
    let mut g = four_pages();
    g.map(0x8000_8000, 4 * P, 0x8000, Permissions::RW).unwrap();
    g.set_u32(0x10ffc, 0xaaaa).unwrap();
    g.set_u32(0x11000, 0xbbbb).unwrap();
    g.set_u32(0x13000, 0xcccc).unwrap();
    g.unmap(0x11000, 2 * P).unwrap();
    assert_eq!(
        g.mappings(),
        &[
            m(0x10000, P, 0x8000, Permissions::RW),
            m(0x13000, P, 0xb000, Permissions::RW),
            m(0x8000_8000, 4 * P, 0x8000, Permissions::RW),
        ]
    );
    assert_eq!(reason(g.u8(0x11000)), FaultReason::Unmapped);
    assert_eq!(reason(g.u8(0x12fff)), FaultReason::Unmapped);
    assert_eq!(reason(g.set_u8(0x12000, 0)), FaultReason::Unmapped);
    // Accesses straddling into the hole fault without partial writes.
    assert_eq!(reason(g.set_u64(0x10ffc, u64::MAX)), FaultReason::Unmapped);
    assert_eq!(g.u32(0x10ffc).unwrap(), 0xaaaa);
    assert_eq!(g.u32(0x13000).unwrap(), 0xcccc);
    // The alias still sees the unmapped page's bytes; the physical page stays
    // in use, so it is not free.
    assert_eq!(g.u32(0x8000_9000).unwrap(), 0xbbbb);
    assert!(!g.physical_free(0x9000, P));
    // Remapping the same physical page elsewhere sees the preserved bytes.
    g.map(0x40000, P, 0x9000, Permissions::READ).unwrap();
    assert_eq!(g.u32(0x40000).unwrap(), 0xbbbb);
    // Range at the very top of the address space.
    g.map(0xffff_e000, 2 * P, 0x100000, Permissions::RW)
        .unwrap();
    g.unmap(0xffff_f000, P).unwrap();
    assert_eq!(reason(g.u8(0xffff_f000)), FaultReason::Unmapped);
    g.set_u8(0xffff_efff, 1).unwrap();
}

#[test]
fn unmapped_physical_is_reused_zeroed_after_fresh_ram_runs_out() {
    let mut g = Guest::new();
    g.map_zeroed(0x1000_0000, RAM_SIZE as u32, Permissions::RW)
        .unwrap();
    assert_eq!(
        g.map_zeroed(0x2000_0000, P, Permissions::RW),
        Err(MapError::OutOfMemory)
    );
    g.set_u32(0x1000_5000, 0x5a5a_5a5a).unwrap();
    g.unmap(0x1000_5000, P).unwrap();
    assert!(g.physical_free(0x5000, P));
    assert_eq!(g.find_free_physical(P, 0..RAM_SIZE as u32, P), Some(0x5000));
    assert_eq!(g.find_free_physical(2 * P, 0..RAM_SIZE as u32, P), None);
    g.map_zeroed(0x2000_0000, P, Permissions::RW).unwrap();
    assert_eq!(g.mappings().last().unwrap().physical_start, 0x5000);
    assert_eq!(g.u32(0x2000_0000).unwrap(), 0, "stale bytes leaked");
    assert_eq!(
        g.map_zeroed(0x2000_1000, P, Permissions::RW),
        Err(MapError::OutOfMemory)
    );
}

#[test]
fn find_free_physical_honours_window_alignment_and_occupancy() {
    let mut g = Guest::new();
    g.map(0x10000, 0x10, 0x3000, Permissions::RW).unwrap(); // occupies the whole page
    assert!(!g.physical_free(0x3800, 0x10));
    assert_eq!(g.find_free_physical(P, 0x3000..0x10000, P), Some(0x4000));
    assert_eq!(
        g.find_free_physical(P, 0x3000..0x10000, 0x8000),
        Some(0x8000)
    );
    assert_eq!(g.find_free_physical(P, 0x3000..0x4000, P), None);
    assert_eq!(g.find_free_physical(0, 0..0x10000, P), None);
    assert_eq!(g.find_free_physical(P, 0..0x10000, 3), None);
    assert_eq!(
        g.find_free_physical(P, RAM_SIZE as u32 - P..u32::MAX, P),
        Some(RAM_SIZE as u32 - P)
    );
    assert!(!g.physical_free(RAM_SIZE as u32 - P, 2 * P));
}

fn reserved() -> (Guest, runtime::guest::ReservationId) {
    let mut g = Guest::new();
    let id = g
        .reserve_physical(ARENA, ARENA_LEN, &[ARENA_VA], "arena")
        .unwrap();
    (g, id)
}

#[test]
fn reservations_cannot_be_violated_by_any_allocator_or_map() {
    let (mut g, id) = reserved();
    // Bump allocation fills below the arena, then skips past it, to the
    // first address congruent with the mapping's modulo 16 KiB.
    g.map_zeroed(0x10000, 0x50000, Permissions::RW).unwrap();
    g.map_zeroed(0x60000, 0x20000, Permissions::RW).unwrap();
    let after = HALO2_PHYSICAL_ARENA.end.next_multiple_of(0x4000);
    assert_eq!(g.mappings()[0].physical_start, 0);
    assert_eq!(g.mappings()[1].physical_start, after);
    // Exhaust everything else; nothing ever lands in the arena.
    let rest = RAM_SIZE as u32 - after - 0x20000;
    g.map_zeroed(0x1000_0000, rest, Permissions::RW).unwrap();
    // The 0x11000-byte gap below the arena and the bytes skipped above it
    // are the only free RAM left.
    assert_eq!(
        g.find_free_physical(P, 0..RAM_SIZE as u32, P),
        Some(0x50000)
    );
    g.map_zeroed(0x0800_0000, 0x11000, Permissions::RW).unwrap();
    let gap = g.mappings().iter().find(|m| m.virtual_start == 0x0800_0000);
    assert_eq!(gap.unwrap().physical_start, 0x50000);
    // With nothing congruent left, the skipped bytes are used as they are.
    let skipped = after - HALO2_PHYSICAL_ARENA.end;
    g.map_zeroed(0x2100_0000, skipped, Permissions::RW).unwrap();
    let gap = g.mappings().iter().find(|m| m.virtual_start == 0x2100_0000);
    assert_eq!(gap.unwrap().physical_start, HALO2_PHYSICAL_ARENA.end);
    assert_eq!(
        g.map_zeroed(0x2000_0000, P, Permissions::RW),
        Err(MapError::OutOfMemory)
    );
    // Fallback reuse also skips the arena: free the low pages and refill.
    g.unmap(0x10000, 0x50000).unwrap();
    assert_eq!(
        g.map_zeroed(0x2000_0000, 0x51000, Permissions::RW),
        Err(MapError::OutOfMemory)
    );
    g.map_zeroed(0x2000_0000, 0x50000, Permissions::RW).unwrap();
    for m in g.mappings() {
        let end = m.physical_start as u64 + m.len as u64;
        assert!(
            end <= ARENA as u64 || m.physical_start >= HALO2_PHYSICAL_ARENA.end,
            "{m:?} entered the arena"
        );
    }
    assert!(!g.physical_free(ARENA, P));
    assert!(!g.physical_free(HALO2_PHYSICAL_ARENA.end - P, P));
    assert_eq!(g.find_free_physical(P, HALO2_PHYSICAL_ARENA, P), None);
    // Explicit maps cannot use reserved physical bytes or alias windows.
    for (va, phys) in [
        (0x3000_0000, ARENA),
        (0x3000_0000, HALO2_PHYSICAL_ARENA.end - 1),
        (0x3000_0000, ARENA - P), // straddles the start
        (ARENA_VA, 0x5000),
        (ARENA_VA + ARENA_LEN - 1, 0x5000),
        (ARENA_VA - P, 0x5000), // straddles the window start
    ] {
        let len = if va == ARENA_VA - P || phys == ARENA - P {
            2 * P
        } else {
            1
        };
        assert_eq!(
            g.map(va, len, phys, Permissions::RW),
            Err(MapError::Reserved),
            "{va:#x} -> {phys:#x}"
        );
    }
    assert_eq!(
        g.map_zeroed(ARENA_VA, P, Permissions::RW),
        Err(MapError::Reserved)
    );
    // Only map_reserved reaches the arena, at the window's own offset.
    g.map_reserved(id, ARENA_VA + P, 2 * P, Permissions::RW)
        .unwrap();
    assert_eq!(
        g.mappings()
            .iter()
            .find(|m| m.virtual_start == ARENA_VA + P),
        Some(&m(ARENA_VA + P, 2 * P, ARENA + P, Permissions::RW))
    );
}

#[test]
fn map_reserved_rejects_anything_outside_its_window() {
    let (mut g, id) = reserved();
    let other = g
        .reserve_physical(0x3200000, 2 * P, &[0x9000_0000], "other")
        .unwrap();
    for (rid, va, len, err) in [
        (id, ARENA_VA - P, 2 * P, MapError::Reserved), // straddles window start
        (id, ARENA_VA + ARENA_LEN - P, 2 * P, MapError::Reserved), // straddles window end
        (id, 0x9000_0000, P, MapError::Reserved),      // another reservation's window
        (other, ARENA_VA, P, MapError::Reserved),
        (id, 0x1000_0000, P, MapError::Reserved),
        (id, ARENA_VA + 0x800, P, MapError::InvalidRange),
        (id, ARENA_VA, 0x800, MapError::InvalidRange),
        (id, ARENA_VA, 0, MapError::InvalidRange),
    ] {
        assert_eq!(
            g.map_reserved(rid, va, len, Permissions::RW),
            Err(err),
            "{va:#x}+{len:#x}"
        );
    }
    assert!(g.mappings().is_empty());
    g.map_reserved(id, ARENA_VA, ARENA_LEN, Permissions::RW)
        .unwrap();
    assert_eq!(
        g.map_reserved(id, ARENA_VA, P, Permissions::RW),
        Err(MapError::Overlap)
    );
    g.map_reserved(other, 0x9000_0000, 2 * P, Permissions::RW)
        .unwrap();
    // Distinct reservations have distinct bytes.
    g.set_u32(ARENA_VA, 1).unwrap();
    assert_eq!(g.u32(0x9000_0000).unwrap(), 0);
}

#[test]
fn reserved_aliases_share_bytes_and_survive_protect_and_unmap() {
    let mut g = Guest::new();
    // Two windows over one reservation: cached and write-combined style views.
    let id = g
        .reserve_physical(ARENA, 4 * P, &[ARENA_VA, 0xf006_1000], "two windows")
        .unwrap();
    g.map_reserved(id, ARENA_VA, 4 * P, Permissions::RW)
        .unwrap();
    g.map_reserved(id, 0xf006_1000, 4 * P, Permissions::RW)
        .unwrap();
    g.set_u32(ARENA_VA + 0x1ffc, 0x0bad_cafe).unwrap();
    assert_eq!(g.u32(0xf006_2ffc).unwrap(), 0x0bad_cafe);
    // The game's XPhysicalProtect on part of its arena.
    g.protect_range(ARENA_VA + P, 2 * P, Permissions::READ)
        .unwrap();
    assert_eq!(reason(g.set_u8(ARENA_VA + P, 0)), FaultReason::Protection);
    g.set_u32(0xf006_2000, 0x600d).unwrap(); // other window still writable
    assert_eq!(g.u32(ARENA_VA + P).unwrap(), 0x600d);
    // MmFreeContiguousMemory-style unmap of the whole window, then remap.
    g.unmap(ARENA_VA, 4 * P).unwrap();
    assert_eq!(reason(g.u8(ARENA_VA)), FaultReason::Unmapped);
    assert!(!g.physical_free(ARENA, P), "physical must stay reserved");
    assert_eq!(
        g.map(ARENA_VA, P, 0x100000, Permissions::RW),
        Err(MapError::Reserved),
        "window must stay reserved"
    );
    g.unmap(0xf006_1000, 4 * P).unwrap();
    assert!(!g.physical_free(ARENA, 4 * P));
    g.map_reserved(id, ARENA_VA, 4 * P, Permissions::RW)
        .unwrap();
    assert_eq!(g.u32(ARENA_VA + 0x1ffc).unwrap(), 0x0bad_cafe);
    assert_eq!(g.u32(ARENA_VA + P).unwrap(), 0x600d);
}

#[test]
fn reserve_physical_is_checked_and_all_or_nothing() {
    let mut g = Guest::new();
    g.map(0x10000, P, 0x2000, Permissions::RW).unwrap();
    g.map(0x8000_5000, P, 0x30000, Permissions::RW).unwrap();
    let id = g
        .reserve_physical(0x100000, 4 * P, &[0x8010_0000], "a")
        .unwrap();
    // Live backing, including the existing reservation, holds sentinels that
    // no rejected reservation may clear.
    g.map_reserved(id, 0x8010_0000, 4 * P, Permissions::RW)
        .unwrap();
    let sentinels = [
        (0x10000, 0x5e57_0001u32),
        (0x10ffc, 0x5e57_0002),
        (0x8000_5000, 0x5e57_0003),
        (0x8000_5ffc, 0x5e57_0004),
        (0x8010_3000, 0x5e57_0005),
        (0x8010_3ffc, 0x5e57_0006),
    ];
    for (va, v) in sentinels {
        g.set_u32(va, v).unwrap();
    }
    let before = g.mappings().to_vec();
    for (phys, len, aliases, err) in [
        (0x1000, 0, vec![], MapError::InvalidRange),
        (0x1800, P, vec![], MapError::InvalidRange),
        (0x1000, 0x800, vec![], MapError::InvalidRange),
        (RAM_SIZE as u32 - P, 2 * P, vec![], MapError::InvalidRange),
        (0x40000, P, vec![0x8004_0800], MapError::InvalidRange),
        (0x40000, 2 * P, vec![0xfcff_f000], MapError::InvalidRange), // NV2A aperture
        (0x40000, 2 * P, vec![0xffff_f000], MapError::InvalidRange), // past 4 GiB
        (0x2000, P, vec![], MapError::Overlap),                      // mapped physical
        (0x40000, 2 * P, vec![0x8000_4000], MapError::Overlap),      // mapped window
        (0x103000, 2 * P, vec![], MapError::Reserved),               // reserved physical
        (0x40000, 2 * P, vec![0x8010_3000], MapError::Reserved),     // reserved window
        (
            0x40000,
            2 * P,
            vec![0x9000_0000, 0x9000_1000],
            MapError::Reserved,
        ), // own windows
    ] {
        assert_eq!(
            g.reserve_physical(phys, len, &aliases, "bad"),
            Err(err),
            "{phys:#x}+{len:#x} {aliases:x?}"
        );
        assert_eq!(g.physical_reservations().len(), 1);
        assert_eq!(g.physical_reservations()[0].id, id);
        assert_eq!(g.mappings(), before.as_slice());
        for (va, v) in sentinels {
            assert_eq!(g.u32(va).unwrap(), v, "{phys:#x}+{len:#x} cleared {va:#x}");
        }
    }
    // A window ending exactly at the NV2A aperture is accepted.
    g.reserve_physical(0x40000, P, &[0xfcff_f000], "edge")
        .unwrap();
    // Reserving zeroes physical bytes a former mapping left behind.
    g.set_u32(0x10000, 0xdead).unwrap();
    g.unmap(0x10000, P).unwrap();
    let z = g.reserve_physical(0x2000, P, &[0x8000_2000], "z").unwrap();
    g.map_reserved(z, 0x8000_2000, P, Permissions::RW).unwrap();
    assert_eq!(g.u32(0x8000_2000).unwrap(), 0);
}

#[test]
fn claims_protect_hle_ranges_from_guest_frees_but_not_from_owners() {
    const HLE: u32 = 0xa000_0000;
    const HLE_LEN: u32 = 0x0100_0000;
    let mut g = Guest::new();
    let claim = g.claim_virtual(HLE, HLE_LEN, "bink arena").unwrap();
    assert_eq!(
        g.claim_virtual(HLE + HLE_LEN - P, 2 * P, "x"),
        Err(MapError::Claimed)
    );
    assert_eq!(
        g.claim_virtual(HLE - P, P + 1, "x"),
        Err(MapError::InvalidRange)
    );
    // Mapping inside a claim keeps working (existing HLE allocators).
    g.map_zeroed(HLE, 2 * P, Permissions::RW).unwrap();
    g.map_zeroed(HLE - P, P, Permissions::RW).unwrap();
    assert_eq!(g.unmap(HLE - P, 2 * P), Err(MapError::Claimed));
    assert_eq!(
        g.protect_range(HLE, P, Permissions::READ),
        Err(MapError::Claimed)
    );
    g.unmap(HLE - P, P).unwrap(); // the unclaimed neighbour is fine
    g.protect_range_owned(claim, HLE + P, P, Permissions::READ)
        .unwrap();
    assert_eq!(reason(g.set_u8(HLE + P, 1)), FaultReason::Protection);
    g.unmap_owned(claim, HLE, P).unwrap();
    assert_eq!(reason(g.u8(HLE)), FaultReason::Unmapped);
    assert_eq!(g.u8(HLE + P).unwrap(), 0);
    assert_eq!(g.virtual_claims().len(), 1);
    assert_eq!(g.virtual_claims()[0].owner, "bink arena");
}

#[test]
fn dispatch_faults_after_execute_is_removed_or_code_is_unmapped() {
    // An executable page outside the image's guarded sections (an HLE
    // adapter page, for example) can lose Execute; dispatch then faults.
    const ADAPTER: u32 = 0x0050_0000;
    for unmap in [false, true] {
        let image = load_xbe(&synthetic_xbe()).unwrap();
        image
            .runtime
            .memory()
            .map_zeroed(ADAPTER, PAGE_SIZE, Permissions::RX)
            .unwrap();
        image.runtime.edit_dispatch(|d| {
            let _ = d.register_override(ADAPTER, "adapter", |cpu, _, _| {
                cpu.eax = 777;
                Ok(TransferAbi::stop(0))
            });
        });
        {
            let mut memory = image.runtime.memory();
            let page = ADAPTER;
            if unmap {
                memory.unmap(page, PAGE_SIZE).unwrap();
            } else {
                memory
                    .protect_range(page, PAGE_SIZE, Permissions::RW)
                    .unwrap();
            }
        }
        let mut ctx = image.context();
        let mut cpu = image.cpu;
        let t = Runner::default().run(&mut cpu, &mut ctx, ADAPTER);
        assert_eq!(t.kind, abi::FAULT);
        assert_eq!(cpu.eax, 0);
        let expected = if unmap {
            FaultReason::Unmapped
        } else {
            FaultReason::Protection
        };
        assert!(
            matches!(ctx.event(t.event_id), Some(Event::MemoryFault { fault, .. })
                if fault.access == Access::Execute && fault.reason == expected),
            "{:?}",
            ctx.event(t.event_id)
        );
    }
}

#[test]
fn lifted_code_cannot_lose_execute_or_be_unmapped() {
    let image = load_xbe(&synthetic_xbe()).unwrap();
    let mut memory = image.runtime.memory();
    let page = ENTRY & !(PAGE_SIZE - 1);
    // The loader guards the executable section containing ENTRY.
    assert!(
        memory
            .code_ranges()
            .iter()
            .any(|r| r.contains(&(page as u64)))
    );
    let before = memory.mappings().to_vec();
    for perms in [Permissions::NONE, Permissions::READ, Permissions::RW] {
        assert_eq!(
            memory.protect_range(page, PAGE_SIZE, perms),
            Err(MapError::LiftedCode)
        );
    }
    assert_eq!(memory.unmap(page, PAGE_SIZE), Err(MapError::LiftedCode));
    // A range that only partly covers code is refused as a whole.
    assert_eq!(
        memory.protect_range(page - PAGE_SIZE, 2 * PAGE_SIZE, Permissions::RW),
        Err(MapError::LiftedCode)
    );
    assert_eq!(memory.mappings(), before.as_slice());
    assert!(memory.check(ENTRY, 1, Access::Execute, 1).is_ok());
    // Keeping Execute is allowed: write-protecting or opening code up.
    memory
        .protect_range(page, PAGE_SIZE, Permissions::RX)
        .unwrap();
    memory
        .protect_range(page, PAGE_SIZE, Permissions::RWX)
        .unwrap();
    drop(memory);
    // Hand-guarded code, and the claim owner is bound by the guard too.
    let mut g = Guest::new();
    let claim = g.claim_virtual(0x10000, 4 * P, "hle").unwrap();
    g.map_zeroed(0x10000, 4 * P, Permissions::RX).unwrap();
    assert_eq!(g.guard_code(0x10000, 0), Err(MapError::InvalidRange));
    assert_eq!(
        g.guard_code(0xffff_f000, 2 * P),
        Err(MapError::InvalidRange)
    );
    g.guard_code(0x11800, 0x10).unwrap(); // rounds out to the page
    assert_eq!(g.code_ranges(), vec![0x11000..0x12000]);
    assert_eq!(g.unmap_owned(claim, 0x11000, P), Err(MapError::LiftedCode));
    assert_eq!(
        g.protect_range_owned(claim, 0x10000, 4 * P, Permissions::READ),
        Err(MapError::LiftedCode)
    );
    // Pages beside the guard are unaffected.
    g.unmap_owned(claim, 0x10000, P).unwrap();
    g.protect_range_owned(claim, 0x12000, P, Permissions::READ)
        .unwrap();
    // Bounds and holes still report before the code guard.
    assert_eq!(
        g.protect_range(0x11000, 0x800, Permissions::READ),
        Err(MapError::InvalidRange)
    );
}

#[test]
fn loader_reserves_before_placing_image_stack_and_data() {
    let plain = load_xbe(&synthetic_xbe()).unwrap();
    assert!(plain.runtime.memory().physical_reservations().is_empty());
    let image = load_xbe_with(&synthetic_xbe(), &LoadOptions::halo2()).unwrap();
    let memory = image.runtime.memory();
    let r = &memory.physical_reservations()[0];
    assert_eq!(
        (r.physical_start, r.len, r.aliases.as_slice()),
        (ARENA, ARENA_LEN, &[ARENA_VA][..])
    );
    assert!(!memory.mappings().is_empty());
    for m in memory.mappings() {
        let end = m.physical_start as u64 + m.len as u64;
        assert!(
            end <= ARENA as u64 || m.physical_start >= HALO2_PHYSICAL_ARENA.end,
            "{m:?} entered the arena"
        );
        assert!(
            m.virtual_start as u64 + m.len as u64 <= ARENA_VA as u64
                || m.virtual_start >= ARENA_VA + ARENA_LEN,
            "{m:?} entered the alias window"
        );
    }
    // Same guest behaviour as the default load.
    assert_eq!(memory.u32(ENTRY).unwrap(), 0xcccc_cccc);
    assert_eq!(
        memory.u32(image.cpu.esp).unwrap(),
        runtime::exec::HARNESS_EXIT
    );
    drop(memory);
    // A window over the DATA export page fails the load instead of moving it.
    let clash = LoadOptions {
        physical_reservations: vec![ReservationRequest {
            physical: 0x0300_0000..0x0300_1000,
            aliases: vec![0x8000_0000],
            owner: "clash",
        }],
    };
    assert!(matches!(
        load_xbe_with(&synthetic_xbe(), &clash),
        Err(LoadError::Mapping(MapError::Reserved))
    ));
    let backwards = LoadOptions {
        physical_reservations: vec![ReservationRequest {
            physical: std::ops::Range {
                start: 0x2000,
                end: 0x1000,
            },
            aliases: vec![],
            owner: "backwards",
        }],
    };
    assert!(matches!(
        load_xbe_with(&synthetic_xbe(), &backwards),
        Err(LoadError::Invalid(_))
    ));
}

#[test]
#[ignore = "requires the user's local disc/default.xbe; never copies game data"]
fn real_disc_arena_reservation_and_physical_budget() {
    // H2_XBE lets a worktree without disc/ read the main checkout's copy.
    let path = std::env::var_os("H2_XBE").map_or_else(
        || std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../disc/default.xbe"),
        std::path::PathBuf::from,
    );
    let bytes = std::fs::read(path).expect("supply disc/default.xbe locally");
    let image = load_xbe_with(&bytes, &LoadOptions::halo2()).unwrap();
    let memory = image.runtime.memory();
    // Decode 0x0012b400..0x0012b445: the arena cursors, the XPhysicalAlloc
    // arguments and the ignored result.
    let mut code = [0u8; 0x45];
    memory.read_bytes(0x0012_b400, &mut code, 1).unwrap();
    let mut decoder = Decoder::with_ip(32, &code, 0x0012_b400, DecoderOptions::NONE);
    let decoded: Vec<Instruction> = decoder.iter().collect();
    let at = |ip: u64| {
        *decoded
            .iter()
            .find(|i| i.ip() == ip)
            .unwrap_or_else(|| panic!("no instruction at {ip:#x}"))
    };
    let mov = |ip, reg, value| {
        let i = at(ip);
        assert_eq!((i.mnemonic(), i.op0_register()), (Mnemonic::Mov, reg));
        assert_eq!(i.op1_kind(), OpKind::Immediate32);
        assert_eq!(i.immediate32(), value, "{ip:#x}");
    };
    mov(0x0012_b411, Register::ECX, ARENA);
    mov(0x0012_b41d, Register::EAX, HALO2_PHYSICAL_ARENA.end);
    // The four pushes, last first: protect, alignment, physical, size.
    let pushes: Vec<_> = decoded
        .iter()
        .filter(|i| i.mnemonic() == Mnemonic::Push && i.ip() >= 0x0012_b403)
        .collect();
    assert_eq!(pushes.len(), 4);
    assert_eq!(pushes[0].immediate32(), 4); // PAGE_READWRITE
    assert_eq!(pushes[1].immediate32(), 0); // alignment
    assert_eq!(pushes[2].op0_register(), Register::ECX); // physical 0x61000
    assert_eq!(pushes[3].immediate32(), ARENA_LEN); // size
    let call = at(0x0012_b43e);
    assert_eq!(call.mnemonic(), Mnemonic::Call);
    assert_eq!(call.near_branch_target(), 0x002d_156a);
    // pop edi; ret: EAX, the allocation result, is never read.
    let after: Vec<_> = decoded.iter().filter(|i| i.ip() > 0x0012_b43e).collect();
    assert_eq!(after[0].mnemonic(), Mnemonic::Pop);
    assert_eq!(after[1].mnemonic(), Mnemonic::Ret);
    assert_eq!(0x0314_5000 - 0x0006_1000, 0x030e_4000);
    // Budget: everything the loader placed fits outside the arena.
    let mut used = 0u64;
    for m in memory.mappings() {
        let end = m.physical_start as u64 + m.len as u64;
        assert!(end <= ARENA as u64 || m.physical_start >= HALO2_PHYSICAL_ARENA.end);
        used += (m.len as u64).div_ceil(PAGE_SIZE as u64) * PAGE_SIZE as u64;
    }
    let outside = RAM_SIZE as u64 - ARENA_LEN as u64;
    let free = outside - used;
    eprintln!("arena {ARENA_LEN:#x}; outside {outside:#x}; loader used {used:#x}; free {free:#x}");
    assert!(free > 0x0080_0000, "less than 8 MiB left outside the arena");
    // The arena itself is still mappable as the game expects.
    drop(memory);
    let mut memory = image.runtime.memory();
    let id = memory.physical_reservations()[0].id;
    memory
        .map_reserved(id, ARENA_VA, ARENA_LEN, Permissions::RW)
        .unwrap();
    memory.set_u32(ARENA_VA, 1).unwrap();
}

#[test]
fn read_append_copies_what_read_bytes_does_and_nothing_on_a_fault() {
    // Two adjacent mappings backed by physical pages out of order.
    let mut g = Guest::new();
    g.map(0x10000, P, 0x9000, Permissions::RW).unwrap();
    g.map(0x11000, P, 0x8000, Permissions::RW).unwrap();
    let bytes: Vec<u8> = (0..2 * P).map(|i| (i * 7 + 3) as u8).collect();
    g.write_bytes(0x10000, &bytes, 1).unwrap();
    g.protect(0x11000, Permissions::READ).unwrap();
    let mut want = vec![0; 0x40];
    g.read_bytes(0x10fe0, &mut want, 1).unwrap();
    let mut out = vec![9u8; 2];
    g.read_append(0x10fe0, 0x40, &mut out, 1).unwrap();
    assert_eq!(out[..2], [9, 9]);
    assert_eq!(out[2..], want);
    assert_eq!(out[2..], bytes[0xfe0..0x1020]);
    // Past the mapped range, misaligned, and empty: `out` is untouched.
    let before = out.clone();
    assert_eq!(
        reason(g.read_append(0x11ff0, 0x20, &mut out, 1)),
        FaultReason::Unmapped
    );
    assert_eq!(
        reason(g.read_append(0x10001, 4, &mut out, 4)),
        FaultReason::Misaligned
    );
    g.read_append(0x10000, 0, &mut out, 1).unwrap();
    assert_eq!(out, before);
}

#[test]
fn hle_outputs_capture_checked_writes_including_fast_and_cross_mapping_accesses() {
    let mut g = Guest::new();
    g.map(0x1000, P, 0, Permissions::RW).unwrap();
    g.map(0x2000, P, 2 * P, Permissions::RW).unwrap();
    g.map(0x3000, P, 4 * P, Permissions::READ).unwrap();
    g.prepare_fast_pages();
    let (_, captured) = g.capture_hle_writes(9, |g| {
        g.write_bytes_from(11, 0x1000, &[5, 6, 7, 8], 4).unwrap();
        g.write_bytes_from(12, 0x1ffe, &[1, 2, 3, 4], 1).unwrap();
        assert!(g.write_bytes_from(13, 0x2ffe, &[9; 4], 1).is_err());
        g.set_u8(0x1001, 0).unwrap();
        g.write_bytes(0x1000, &[], 1).unwrap();
    });
    let captured = captured.unwrap();
    assert!(captured.zeroed.is_empty());
    assert_eq!(
        captured.writes,
        [
            (0x1000, vec![5, 6, 7, 8]),
            (0x1ffe, vec![1, 2, 3, 4]),
            (0x1001, vec![0]),
        ]
    );
    assert_eq!(g.u16(0x2ffe).unwrap(), 0);
    let (_, oversized) = g.capture_hle_writes(3, |g| g.set_u32(0x1000, 55).unwrap());
    assert!(oversized.is_none());
    assert_eq!(g.u32(0x1000).unwrap(), 55);
}

#[test]
fn capturing_reused_zeroed_storage_records_the_allocation_without_copying_its_bytes() {
    let mut g = Guest::new();
    g.map(0x1000, P, RAM_SIZE as u32 - P, Permissions::RW)
        .unwrap();
    g.map(0x2000, P, 0, Permissions::RW).unwrap();
    g.set_u32(0x2000, u32::MAX).unwrap();
    g.unmap(0x2000, P).unwrap();
    let (_, captured) = g.capture_hle_writes(0, |g| {
        g.map_zeroed(0x3000, P, Permissions::RW).unwrap();
    });
    let captured = captured.unwrap();
    assert_eq!(captured.zeroed, [(0x3000, P, Permissions::RW)]);
    assert!(captured.writes.is_empty());
    assert_eq!(g.u32(0x3000).unwrap(), 0);
}

#[test]
fn capture_rejects_unreproducible_layout_changes_even_if_a_known_allocation_follows() {
    let mut g = four_pages();
    let (_, captured) = g.capture_hle_writes(64, |g| {
        g.protect_range(0x11000, P, Permissions::READ).unwrap();
        g.map_zeroed(0x20000, P, Permissions::RW).unwrap();
    });
    assert!(captured.is_none());
    let (_, captured) = g.capture_hle_writes(64, |g| {
        g.reserve_physical(0, P, &[], "test").unwrap();
    });
    assert!(captured.is_none());
    let (_, recovered) = g.capture_hle_writes(1, |g| g.set_u8(0x10000, 9).unwrap());
    assert_eq!(recovered.unwrap().writes, [(0x10000, vec![9])]);
}

/// The entry table re-reads execute bits only when `code_id` moves
/// (`runtime::direct`), so every change that can take Execute from lifted
/// code must move it, and the ones that cannot must leave it.
#[test]
fn code_id_moves_exactly_when_lifted_code_can_lose_execute() {
    let mut g = Guest::new();
    g.map_zeroed(0x10000, 4 * P, Permissions::RX).unwrap();
    g.map_zeroed(0x20000, P, Permissions::RW).unwrap();
    let before = g.code_id();
    g.guard_code(0x11000, P).unwrap();
    let guarded = g.code_id();
    assert_ne!(guarded, before, "a guard moves it");
    // Range changes cannot take Execute from lifted code, and a
    // whole-mapping protect elsewhere does not touch it.
    g.protect_range(0x10000, P, Permissions::READ).unwrap();
    g.protect(0x20000, Permissions::READ).unwrap();
    assert_eq!(g.code_id(), guarded);
    // A whole-mapping protect over lifted code does, even keeping Execute.
    g.protect(0x10000, Permissions::RX).unwrap();
    let protected = g.code_id();
    assert_ne!(protected, guarded);
    // Restoring a snapshot layout moves it too.
    let layout = g.layout();
    g.restore(&layout, &vec![0u8; RAM_SIZE]).unwrap();
    assert_ne!(g.code_id(), protected);
}
