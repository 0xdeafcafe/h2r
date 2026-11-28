use super::*;
use guest_layout::FlatRam;

/// The pool header, the region and the words the stamp's caller word reads.
const P: u32 = 0x004f_0000;
const R: u32 = 0x0050_0000;
const SIZE: u32 = 0x1000;
const OWNERS: u32 = 0x0042_0000;
const TAG: u32 = 7;

fn word(ram: &FlatRam, at: u32) -> u32 {
    rd(ram, at).unwrap()
}

fn put(ram: &mut FlatRam, at: u32, v: u32) {
    wr(ram, at, v).unwrap();
}

/// A pool over `[R, R + SIZE)` with the given flags.
fn pool(stamped: bool, gaps: bool, fixed: bool) -> FlatRam {
    let mut ram = FlatRam::new(0x0041_0000, 0x0020_0000);
    put(&mut ram, P + field::BASE, R);
    put(&mut ram, P + field::SIZE, SIZE);
    put(&mut ram, P + field::FREE, SIZE);
    for (off, on) in [(field::FIXED, fixed), (field::FILL_GAPS, gaps), (field::STAMPED, stamped)] {
        ram.write_bytes(P + off, &[u8::from(on)]).unwrap();
    }
    // `3314b0`: the word at `[[411690]]`.
    put(&mut ram, CALLER_TAG, 0x0041_1700);
    put(&mut ram, 0x0041_1700, 0xcafe);
    ram
}

/// Allocate a body of `size` bytes whose owner is `OWNERS + 4 * n`; the
/// body's address, or none.
fn alloc_n(ram: &mut FlatRam, n: u32, size: u32) -> Option<u32> {
    let owner = OWNERS + 4 * n;
    alloc(ram, P, owner, size, TAG).unwrap().then(|| word(ram, owner))
}

/// `bc300`'s cut of one block out of the list, as the lifted function does.
fn free_body(ram: &mut FlatRam, body: u32) {
    let block = body - 0x10;
    let free = word(ram, P + field::FREE) + word(ram, block);
    put(ram, P + field::FREE, free);
    let (next, prev) = (word(ram, block + 0x0c), word(ram, block + 8));
    put(ram, if next != 0 { next + 8 } else { P + field::LOWEST }, prev);
    put(ram, if prev != 0 { prev + 0x0c } else { P + field::HIGHEST }, next);
}

fn header(ram: &FlatRam) -> [u32; 3] {
    [P + field::FREE, P + field::LOWEST, P + field::HIGHEST].map(|at| word(ram, at))
}

#[test]
fn the_first_block_is_the_regions_base() {
    let mut ram = pool(false, false, false);
    // 0x24 + the 0x10 header, rounded up to 16.
    assert_eq!(alloc_n(&mut ram, 0, 0x24), Some(R + 0x10));
    assert_eq!(word(&ram, R), 0x40);
    assert_eq!(word(&ram, R + 4), OWNERS);
    assert_eq!((word(&ram, R + 8), word(&ram, R + 0x0c)), (0, 0));
    assert_eq!(header(&ram), [SIZE - 0x40, R, R]);
}

#[test]
fn the_next_block_goes_above_and_links_both_ways() {
    let mut ram = pool(false, false, false);
    alloc_n(&mut ram, 0, 0x24).unwrap();
    assert_eq!(alloc_n(&mut ram, 1, 0x30), Some(R + 0x50));
    // Exactly 0x40: no rounding.
    assert_eq!(word(&ram, R + 0x40), 0x40);
    assert_eq!((word(&ram, R + 8), word(&ram, R + 0x40 + 0x0c)), (R + 0x40, R));
    assert_eq!(header(&ram), [SIZE - 0x80, R, R + 0x40]);
}

#[test]
fn a_stamped_pool_puts_the_stamp_before_each_block() {
    let mut ram = pool(true, false, false);
    // 0x24 + 0x20, rounded up to 16: 0x50, the block 0x10 into it.
    assert_eq!(alloc_n(&mut ram, 0, 0x24), Some(R + 0x20));
    assert_eq!([word(&ram, R), word(&ram, R + 4), word(&ram, R + 8), word(&ram, R + 0x0c)], [STAMP, OWNERS, TAG, 0xcafe]);
    assert_eq!(word(&ram, R + 0x10), 0x50);
    assert_eq!(header(&ram), [SIZE - 0x50, R + 0x10, R + 0x10]);
    // The next block starts where the first one's size ends.
    assert_eq!(alloc_n(&mut ram, 1, 0x24), Some(R + 0x50 + 0x20));
    assert_eq!(word(&ram, R + 0x50), STAMP);
    assert_eq!(word(&ram, R + 0x60), 0x50);
}

#[test]
fn a_fixed_pool_keeps_no_owner() {
    let mut ram = pool(false, false, true);
    assert_eq!(alloc_n(&mut ram, 0, 0x20), Some(R + 0x10));
    assert_eq!(word(&ram, R + 4), 0);
}

#[test]
fn a_full_pool_answers_false_and_changes_nothing() {
    let mut ram = pool(false, false, false);
    alloc_n(&mut ram, 0, 0x20).unwrap();
    let before = ram.clone();
    assert_eq!(alloc_n(&mut ram, 1, SIZE), None);
    assert_eq!(ram, before);
    // What exactly fits does: the first block took 0x30, the rest is 0xfd0 with its header.
    assert!(alloc(&mut ram, P, OWNERS + 8, SIZE - 0x30 - 0x10, TAG).unwrap());
    assert_eq!(word(&ram, P + field::FREE), 0);
}

#[test]
fn alloc_then_free_round_trips_the_header() {
    let mut ram = pool(false, false, false);
    let start = header(&ram);
    let body = alloc_n(&mut ram, 0, 0x100).unwrap();
    free_body(&mut ram, body);
    assert_eq!(header(&ram), [start[0], 0, 0]);
    // The next allocation lands where the first did.
    assert_eq!(alloc_n(&mut ram, 1, 0x100), Some(body));
}

#[test]
fn a_gap_pool_fills_the_first_gap_that_holds_the_block() {
    let mut ram = pool(false, true, false);
    let a = alloc_n(&mut ram, 0, 0x30).unwrap();
    let b = alloc_n(&mut ram, 1, 0x30).unwrap();
    let c = alloc_n(&mut ram, 2, 0x30).unwrap();
    assert_eq!([a, b, c], [R + 0x10, R + 0x50, R + 0x90]);
    free_body(&mut ram, b);
    // 0x20 + 0x10 = 0x30 fits the 0x40 gap, at the end of `a`.
    assert_eq!(alloc_n(&mut ram, 3, 0x20), Some(R + 0x50));
    assert_eq!(word(&ram, R + 0x40), 0x30);
    assert_eq!((word(&ram, R + 0x40 + 8), word(&ram, R + 0x40 + 0x0c)), (R + 0x80, R));
    assert_eq!((word(&ram, R + 8), word(&ram, R + 0x80 + 0x0c)), (R + 0x40, R + 0x40));
    // Another 0x40 no longer fits the 0x10 left: it goes above the highest.
    assert_eq!(alloc_n(&mut ram, 4, 0x30), Some(R + 0xc0 + 0x10));
    assert_eq!(word(&ram, P + field::HIGHEST), R + 0xc0);
}

#[test]
fn a_gap_pool_fills_below_the_lowest_block() {
    let mut ram = pool(false, true, false);
    let a = alloc_n(&mut ram, 0, 0x30).unwrap();
    alloc_n(&mut ram, 1, 0x30).unwrap();
    free_body(&mut ram, a);
    // The lowest block is now at `R + 0x40`; a 0x30 block fits below it.
    assert_eq!(alloc_n(&mut ram, 2, 0x20), Some(R + 0x10));
    assert_eq!((word(&ram, R + 8), word(&ram, R + 0x0c)), (R + 0x40, 0));
    assert_eq!(word(&ram, R + 0x40 + 0x0c), R);
    assert_eq!(header(&ram)[1..], [R, R + 0x40]);
}

#[test]
fn a_pool_without_gaps_ignores_them() {
    let mut ram = pool(false, false, false);
    let a = alloc_n(&mut ram, 0, 0x30).unwrap();
    alloc_n(&mut ram, 1, 0x30).unwrap();
    free_body(&mut ram, a);
    assert_eq!(alloc_n(&mut ram, 2, 0x20), Some(R + 0x80 + 0x10));
}

fn fill(ram: &mut FlatRam, at: u32, len: usize, byte: u8) {
    ram.write_bytes(at, &vec![byte; len]).unwrap();
}

#[test]
fn compact_slides_later_blocks_down_and_rewrites_their_owners() {
    let mut ram = pool(false, false, false);
    let a = alloc_n(&mut ram, 0, 0x30).unwrap();
    let b = alloc_n(&mut ram, 1, 0x30).unwrap();
    let c = alloc_n(&mut ram, 2, 0x30).unwrap();
    let d = alloc_n(&mut ram, 3, 0x30).unwrap();
    for (body, byte) in [(a, 0xa1), (c, 0xc3), (d, 0xd4)] {
        fill(&mut ram, body, 0x30, byte);
    }
    let free = word(&ram, P + field::FREE);
    free_body(&mut ram, b);
    compact(&mut ram, P).unwrap();
    // `a` stays; `c` and `d` slide down to follow it.
    assert_eq!(word(&ram, OWNERS), a);
    assert_eq!((word(&ram, OWNERS + 8), word(&ram, OWNERS + 12)), (R + 0x50, R + 0x90));
    for (body, byte) in [(a, 0xa1u8), (R + 0x50, 0xc3), (R + 0x90, 0xd4)] {
        let mut got = [0u8; 0x30];
        Mem::read_bytes(&ram, body, &mut got).unwrap();
        assert_eq!(got, [byte; 0x30]);
    }
    // The links: a <-> c <-> d, a the lowest, d the highest.
    assert_eq!((word(&ram, R + 0x0c), word(&ram, R + 8)), (0, R + 0x40));
    assert_eq!((word(&ram, R + 0x40 + 0x0c), word(&ram, R + 0x40 + 8)), (R, R + 0x80));
    assert_eq!((word(&ram, R + 0x80 + 0x0c), word(&ram, R + 0x80 + 8)), (R + 0x40, 0));
    assert_eq!(header(&ram), [free + 0x40, R, R + 0x80]);
    // The owners kept in the moved blocks are theirs.
    assert_eq!((word(&ram, R + 0x40 + 4), word(&ram, R + 0x80 + 4)), (OWNERS + 8, OWNERS + 12));
}

#[test]
fn compact_slides_the_first_block_to_the_base_and_is_idempotent() {
    let mut ram = pool(false, false, false);
    let a = alloc_n(&mut ram, 0, 0x30).unwrap();
    alloc_n(&mut ram, 1, 0x30).unwrap();
    free_body(&mut ram, a);
    compact(&mut ram, P).unwrap();
    assert_eq!(word(&ram, OWNERS + 4), R + 0x10);
    assert_eq!(header(&ram)[1..], [R, R]);
    let once = ram.clone();
    compact(&mut ram, P).unwrap();
    assert_eq!(ram, once);
}

#[test]
fn compact_moves_a_stamp_with_its_block() {
    let mut ram = pool(true, false, false);
    let a = alloc_n(&mut ram, 0, 0x20).unwrap();
    let b = alloc_n(&mut ram, 1, 0x20).unwrap();
    fill(&mut ram, b, 0x20, 0xb2);
    free_body(&mut ram, a);
    compact(&mut ram, P).unwrap();
    // Block size 0x40 (0x20 + 0x20 rounded); the stamp at the base, the block at +0x10.
    assert_eq!(word(&ram, OWNERS + 4), R + 0x20);
    assert_eq!([word(&ram, R), word(&ram, R + 4)], [STAMP, OWNERS + 4]);
    assert_eq!((word(&ram, R + 0x10), word(&ram, R + 0x10 + 8), word(&ram, R + 0x10 + 0x0c)), (0x40, 0, 0));
    assert_eq!(header(&ram)[1..], [R + 0x10, R + 0x10]);
    let mut got = [0u8; 0x20];
    Mem::read_bytes(&ram, R + 0x20, &mut got).unwrap();
    assert_eq!(got, [0xb2; 0x20]);
}

#[test]
fn compact_leaves_a_fixed_or_empty_pool_alone() {
    let mut ram = pool(false, false, false);
    let empty = ram.clone();
    compact(&mut ram, P).unwrap();
    assert_eq!(ram, empty);
    let mut ram = pool(false, false, true);
    let a = alloc_n(&mut ram, 0, 0x30).unwrap();
    alloc_n(&mut ram, 1, 0x30).unwrap();
    free_body(&mut ram, a);
    let before = ram.clone();
    compact(&mut ram, P).unwrap();
    assert_eq!(ram, before);
}
