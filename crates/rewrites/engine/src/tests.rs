//! Behaviour tests over synthetic guest memory. No game data: every
//! structure is built here from the recovered layouts.

use guest_layout::abi::{self, Regs};
use guest_layout::datum::{DataIterator, DatumArray, SALT_WRAP, allocation_size, flags, layout};
use guest_layout::handle::DatumHandle;
use guest_layout::locale::{EMPTY_STRING, LocaleTable, StringId, layout as loc};
use guest_layout::memory::{FlatMemory, GuestMemory, MemFault};
use guest_layout::tags::{
    GroupTag, TAG_ENTRIES_GLOBAL, TAG_INDEX_GLOBAL, tag_get_checked, tag_group_find,
};

// ---- datum arrays ---------------------------------------------------------

const BASE: u32 = 0x0010_0000;
const NAME: u32 = BASE;
const ARRAY: u32 = BASE + 0x100;

/// A made-valid contiguous array named `name` with `count` elements of
/// `size` bytes, built by the same calls the engine's constructors make.
fn array(name: &[u8], count: i32, size: i32) -> (FlatMemory, DatumArray) {
    let mut m = FlatMemory::new(BASE, 0x10000);
    m.write(NAME, name).unwrap();
    m.write(NAME + name.len() as u32, &[0]).unwrap();
    let a = DatumArray::at(ARRAY);
    a.initialize_contiguous(&mut m, NAME, count, size, 0, 0)
        .unwrap();
    let fl = m.u8(ARRAY + layout::FLAGS).unwrap();
    m.set_u8(ARRAY + layout::FLAGS, fl | flags::CONTIGUOUS)
        .unwrap();
    a.make_valid(&mut m).unwrap();
    (m, a)
}

fn seed(name: &[u8]) -> u16 {
    u16::from_le_bytes([name[0], name[1]]) | 0x8000
}

#[test]
fn contiguous_layout_and_header() {
    let (m, a) = array(b"ab", 10, 0x14);
    let h = a.header(&m).unwrap();
    assert_eq!(&h.name[..3], b"ab\0");
    assert_eq!((h.maximum_count, h.element_size), (10, 0x14));
    assert_eq!(h.signature, 0x6440_7440);
    assert!(h.valid);
    assert_eq!(h.flags, flags::CONTIGUOUS);
    assert_eq!(h.data, ARRAY + 0x4c);
    assert_eq!(h.bitmap, ARRAY + 0x4c + 10 * 0x14);
    assert_eq!((h.free_scan_start, h.high_water, h.active_count), (0, 0, 0));
    assert_eq!(h.next_salt, seed(b"ab"));
}

#[test]
fn alignment_rounds_the_data_pointer() {
    let mut m = FlatMemory::new(BASE, 0x10000);
    let a = DatumArray::at(ARRAY + 4);
    a.initialize_contiguous(&mut m, NAME, 3, 8, 4, 0x1234)
        .unwrap();
    let h = a.header(&m).unwrap();
    assert_eq!(h.data, (ARRAY + 4 + 0x4c + 15) & !15);
    assert_eq!(h.allocator, 0x1234);
    assert_eq!(h.alignment_bits, 4);
    assert_eq!(allocation_size(3, 8, 4), 24 + 4 + 16 + 0x4b);
}

#[test]
fn single_char_and_empty_names_seed_the_salt() {
    let (m, a) = array(b"x", 2, 4);
    assert_eq!(a.header(&m).unwrap().next_salt, 0x8000 | b'x' as u16);
    let (m, a) = array(b"", 2, 4);
    assert_eq!(a.header(&m).unwrap().next_salt, 0x8000);
}

#[test]
fn new_datum_hands_out_slots_and_salts_in_order() {
    let (mut m, a) = array(b"ab", 4, 8);
    let s = seed(b"ab");
    let h0 = a.new_datum(&mut m).unwrap().unwrap();
    let h1 = a.new_datum(&mut m).unwrap().unwrap();
    assert_eq!(h0, DatumHandle::new(s, 0));
    assert_eq!(h1, DatumHandle::new(s + 1, 1));
    let h = a.header(&m).unwrap();
    assert_eq!(
        (h.free_scan_start, h.high_water, h.active_count, h.next_salt),
        (2, 2, 2, s + 2)
    );
    assert_eq!(m.u32(h.bitmap).unwrap(), 0b11);
    // The element is zeroed apart from its salt.
    let e = a.try_get(&m, h1).unwrap().unwrap();
    assert_eq!(m.u16(e).unwrap(), s + 1);
    assert_eq!(m.u16(e + 2).unwrap(), 0);
}

#[test]
fn new_datum_zeroes_reused_storage() {
    let (mut m, a) = array(b"ab", 2, 8);
    let h = a.new_datum(&mut m).unwrap().unwrap();
    let e = a.try_get(&m, h).unwrap().unwrap();
    m.write(e + 2, &[0xee; 6]).unwrap();
    a.delete(&mut m, h).unwrap();
    let h2 = a.new_datum(&mut m).unwrap().unwrap();
    assert_eq!(h2.index(), 0);
    assert_eq!(a.try_get(&m, h2).unwrap(), Some(e));
    assert_eq!(&m.bytes[(e - BASE) as usize + 2..][..6], &[0; 6]);
}

#[test]
fn deleted_slot_is_reused_with_a_new_salt_and_old_handle_goes_stale() {
    let (mut m, a) = array(b"ab", 4, 8);
    let h0 = a.new_datum(&mut m).unwrap().unwrap();
    let h1 = a.new_datum(&mut m).unwrap().unwrap();
    assert_eq!(a.delete(&mut m, h0).unwrap(), 1);
    assert_eq!(a.header(&m).unwrap().free_scan_start, 0);
    let again = a.new_datum(&mut m).unwrap().unwrap();
    assert_eq!(again.index(), 0);
    assert_ne!(again.salt(), h0.salt());
    assert_eq!(a.try_get(&m, h0).unwrap(), None, "stale handle");
    assert!(a.try_get(&m, again).unwrap().is_some());
    assert!(a.try_get(&m, h1).unwrap().is_some());
}

#[test]
fn full_array_returns_none_and_changes_nothing() {
    let (mut m, a) = array(b"ab", 3, 4);
    for _ in 0..3 {
        assert!(a.new_datum(&mut m).unwrap().is_some());
    }
    let before = m.bytes.clone();
    assert_eq!(a.new_datum(&mut m).unwrap(), None);
    assert_eq!(m.bytes, before);
    // A hole below the high-water mark is found through the bitmap.
    a.delete(&mut m, DatumHandle::new(0, 1)).unwrap();
    assert_eq!(a.new_datum(&mut m).unwrap().unwrap().index(), 1);
}

#[test]
fn salts_skip_ffff_and_wrap_to_8000() {
    let (mut m, a) = array(b"ab", 4, 4);
    m.set_u16(ARRAY + layout::NEXT_SALT, 0xfffe).unwrap();
    assert_eq!(a.new_datum(&mut m).unwrap().unwrap().salt(), 0xfffe);
    assert_eq!(a.header(&m).unwrap().next_salt, SALT_WRAP);
    assert_eq!(a.new_datum(&mut m).unwrap().unwrap().salt(), 0x8000);
}

#[test]
fn try_get_rejects_invalid_handles() {
    let (mut m, a) = array(b"ab", 8, 4);
    let h = a.new_datum(&mut m).unwrap().unwrap();
    assert_eq!(a.try_get(&m, DatumHandle::NONE).unwrap(), None);
    assert_eq!(
        a.try_get(&m, DatumHandle::new(h.salt() ^ 1, 0)).unwrap(),
        None,
        "wrong salt"
    );
    assert_eq!(
        a.try_get(&m, DatumHandle::new(h.salt(), 1)).unwrap(),
        None,
        "past high water"
    );
    assert_eq!(
        a.try_get(&m, DatumHandle::new(0, 0)).unwrap(),
        None,
        "salt 0 never matches"
    );
    a.delete(&mut m, h).unwrap();
    assert_eq!(a.try_get(&m, h).unwrap(), None, "freed");
}

#[test]
fn absolute_lookups() {
    let (mut m, a) = array(b"ab", 8, 4);
    let h = a.new_datum(&mut m).unwrap().unwrap();
    assert!(a.get_absolute(&m, 0).unwrap().is_some());
    assert_eq!(a.get_absolute(&m, -1).unwrap(), None);
    assert_eq!(a.get_absolute(&m, 1).unwrap(), None);
    assert_eq!(a.handle_of(&m, 0).unwrap(), Some(h));
    assert_eq!(a.handle_of(&m, -1).unwrap(), None);
    // No bounds check: slot 5 has never been used and has salt 0.
    assert_eq!(a.handle_of(&m, 5).unwrap(), Some(DatumHandle::new(0, 5)));
}

#[test]
fn delete_lowers_high_water_past_free_slots() {
    let (mut m, a) = array(b"ab", 8, 4);
    let hs: Vec<_> = (0..5)
        .map(|_| a.new_datum(&mut m).unwrap().unwrap())
        .collect();
    a.delete(&mut m, hs[2]).unwrap();
    a.delete(&mut m, hs[3]).unwrap();
    assert_eq!(a.header(&m).unwrap().high_water, 5);
    a.delete(&mut m, hs[4]).unwrap();
    let h = a.header(&m).unwrap();
    assert_eq!((h.high_water, h.active_count, h.free_scan_start), (2, 2, 2));
    a.delete(&mut m, hs[1]).unwrap();
    a.delete(&mut m, hs[0]).unwrap();
    assert_eq!(a.header(&m).unwrap().high_water, 0);
}

#[test]
fn delete_is_unchecked() {
    let (mut m, a) = array(b"ab", 4, 4);
    let old = a.new_datum(&mut m).unwrap().unwrap();
    a.delete(&mut m, old).unwrap();
    let new = a.new_datum(&mut m).unwrap().unwrap();
    // A stale handle frees the slot's new occupant.
    a.delete(&mut m, old).unwrap();
    assert_eq!(a.try_get(&m, new).unwrap(), None);
    // Deleting a free slot still decrements the live count.
    assert_eq!(a.delete(&mut m, old).unwrap(), -1);
}

#[test]
fn debug_fill_marks_freed_elements() {
    let (mut m, a) = array(b"ab", 2, 8);
    let fl = m.u8(ARRAY + layout::FLAGS).unwrap();
    m.set_u8(ARRAY + layout::FLAGS, fl | flags::DEBUG_FILL)
        .unwrap();
    let h = a.new_datum(&mut m).unwrap().unwrap();
    let e = a.try_get(&m, h).unwrap().unwrap();
    a.delete(&mut m, h).unwrap();
    assert_eq!(m.u16(e).unwrap(), 0);
    assert_eq!(m.u32(e + 4).unwrap(), 0xbaba_baba);
}

#[test]
fn new_at_index_uses_the_requested_salt() {
    let (mut m, a) = array(b"ab", 4, 4);
    let s = seed(b"ab");
    let want = DatumHandle::new(0x1234, 2);
    assert_eq!(a.new_datum_at(&mut m, want).unwrap(), Some(want));
    let h = a.header(&m).unwrap();
    assert_eq!(h.next_salt, s + 1, "a salt is consumed anyway");
    assert_eq!(h.free_scan_start, 0, "scan start not moved");
    assert_eq!((h.high_water, h.active_count), (3, 1));
    assert!(a.try_get(&m, want).unwrap().is_some());
    assert_eq!(a.new_datum_at(&mut m, want).unwrap(), None, "occupied");
    assert_eq!(
        a.new_datum_at(&mut m, DatumHandle::new(1, 4)).unwrap(),
        None,
        "past capacity"
    );
    // datum_new then fills the hole at 0 first.
    assert_eq!(a.new_datum(&mut m).unwrap().unwrap().index(), 0);
}

#[test]
fn new_at_absolute_index() {
    let (mut m, a) = array(b"ab", 4, 4);
    let h = a.new_datum_at_absolute(&mut m, 3).unwrap().unwrap();
    assert_eq!(h, DatumHandle::new(seed(b"ab"), 3));
    assert_eq!(a.new_datum_at_absolute(&mut m, 3).unwrap(), None);
    assert_eq!(a.new_datum_at_absolute(&mut m, -1).unwrap(), None);
    assert_eq!(a.new_datum_at_absolute(&mut m, 4).unwrap(), None);
}

#[test]
fn iteration_forwards_and_backwards() {
    let (mut m, a) = array(b"ab", 8, 4);
    let hs: Vec<_> = (0..6)
        .map(|_| a.new_datum(&mut m).unwrap().unwrap())
        .collect();
    for i in [0, 2, 3] {
        a.delete(&mut m, hs[i]).unwrap();
    }
    let live = [hs[1], hs[4], hs[5]];

    let mut fwd = vec![];
    let mut cur = None;
    while let Some(h) = a.next_datum(&m, cur).unwrap() {
        fwd.push(h);
        cur = Some(h);
    }
    assert_eq!(fwd, live);
    assert_eq!(a.last_datum(&m).unwrap(), Some(hs[5]));

    let mut back = vec![];
    let mut cur = None;
    while let Some(h) = a.previous_datum(&m, cur).unwrap() {
        back.push(h);
        cur = Some(h);
    }
    back.reverse();
    assert_eq!(back, live);

    assert_eq!(a.next_index(&m, 2).unwrap(), Some(4));
    assert_eq!(a.next_index(&m, -1).unwrap(), None);
    assert_eq!(a.next_index(&m, 6).unwrap(), None);

    // The guest iterator struct.
    let it = DataIterator::at(BASE + 0x40);
    m.set_u32(it.addr, ARRAY).unwrap();
    m.set_u32(it.addr + 4, u32::MAX).unwrap();
    m.set_i32(it.addr + 8, -1).unwrap();
    let mut seen = vec![];
    while let Some(e) = it.next(&mut m).unwrap() {
        seen.push(DatumHandle(m.u32(it.addr + 4).unwrap()));
        assert_eq!(a.try_get(&m, *seen.last().unwrap()).unwrap(), Some(e));
    }
    assert_eq!(seen, live);
    assert_eq!(m.i32(it.addr + 8).unwrap(), 8, "ends at capacity");
    assert_eq!(m.u32(it.addr + 4).unwrap(), u32::MAX);
}

#[test]
fn empty_array_iteration() {
    let (m, a) = array(b"ab", 4, 4);
    assert_eq!(a.next_datum(&m, None).unwrap(), None);
    assert_eq!(a.last_datum(&m).unwrap(), None);
    assert_eq!(a.previous_datum(&m, None).unwrap(), None);
}

#[test]
fn previous_datum_reads_one_element_below_the_data() {
    // Data at the very start of mapped memory: the guest's extra read at
    // element -1 faults.
    let mut m = FlatMemory::new(BASE, 0x10000);
    let a = DatumArray::at(BASE + 0x8000);
    m.set_u32(a.addr + layout::BITMAP, BASE + 0x9000).unwrap();
    m.set_i32(a.addr + layout::ELEMENT_SIZE, 4).unwrap();
    a.attach(&mut m, 4, BASE).unwrap();
    m.set_i32(a.addr + layout::HIGH_WATER, 2).unwrap();
    assert_eq!(
        a.previous_datum(&m, None),
        Err(MemFault {
            addr: BASE - 4,
            len: 2,
            write: false
        })
    );
}

#[test]
fn delete_all_restarts_salts() {
    let (mut m, a) = array(b"ab", 4, 4);
    let first = a.new_datum(&mut m).unwrap().unwrap();
    a.new_datum(&mut m).unwrap();
    a.delete_all(&mut m).unwrap();
    let h = a.header(&m).unwrap();
    assert_eq!((h.high_water, h.active_count, h.free_scan_start), (0, 0, 0));
    assert_eq!(m.u32(h.bitmap).unwrap(), 0);
    // Same name, same seed: an old handle matches its slot's new occupant.
    let again = a.new_datum(&mut m).unwrap().unwrap();
    assert_eq!(again, first);
}

#[test]
fn attach_rebuilds_bookkeeping_from_salts() {
    let (mut m, a) = array(b"ab", 6, 4);
    let data = a.header(&m).unwrap().data;
    for (i, s) in [(1, 0x8111u16), (2, 0x8222), (4, 0x8444)] {
        m.set_u16(data + i * 4, s).unwrap();
    }
    a.attach(&mut m, 6, data).unwrap();
    let h = a.header(&m).unwrap();
    assert_eq!((h.high_water, h.active_count, h.free_scan_start), (5, 3, 0));
    assert_eq!(m.u32(h.bitmap).unwrap(), 0b10110);
    assert!(
        a.try_get(&m, DatumHandle::new(0x8444, 4))
            .unwrap()
            .is_some()
    );
    assert_eq!(a.new_datum(&mut m).unwrap().unwrap().index(), 0);
}

// ---- tags -----------------------------------------------------------------

const INDEX: u32 = 0x0050_0000;
const GROUPS: u32 = INDEX + 0x20;
const ENTRIES: u32 = 0x0050_1000;

fn g(s: &[u8; 4]) -> GroupTag {
    GroupTag::from_chars(*s)
}

/// A published index with sorted groups and three tag slots: `scnr` at 0,
/// `bipd` at 1, and an empty slot 2.
fn tag_index(base: u32) -> FlatMemory {
    let mut m = FlatMemory::new(base, (0x0056_0000 - base) as usize);
    let none = GroupTag::NONE;
    let groups = [
        (g(b"bipd"), g(b"unit"), g(b"obje")),
        (g(b"obje"), none, none),
        (g(b"scnr"), none, none),
        (g(b"unit"), g(b"obje"), none),
    ];
    for (i, (a, b, c)) in groups.iter().enumerate() {
        let r = GROUPS + i as u32 * 12;
        m.set_u32(r, a.0).unwrap();
        m.set_u32(r + 4, b.0).unwrap();
        m.set_u32(r + 8, c.0).unwrap();
    }
    m.set_u32(INDEX, GROUPS).unwrap();
    m.set_u32(INDEX + 4, groups.len() as u32).unwrap();
    m.set_u32(INDEX + 8, ENTRIES).unwrap();
    m.set_u32(INDEX + 0x18, 3).unwrap();
    let entries = [
        (g(b"scnr").0, 0xe174_0000u32, 0x1000_0000u32),
        (g(b"bipd").0, 0xe175_0001, 0x1000_2000),
        (u32::MAX, u32::MAX, 0),
    ];
    for (i, (grp, datum, addr)) in entries.iter().enumerate() {
        let e = ENTRIES + i as u32 * 16;
        m.set_u32(e, *grp).unwrap();
        m.set_u32(e + 4, *datum).unwrap();
        m.set_u32(e + 8, *addr).unwrap();
    }
    m.set_u32(TAG_INDEX_GLOBAL, INDEX).unwrap();
    m.set_u32(TAG_ENTRIES_GLOBAL, ENTRIES).unwrap();
    m
}

#[test]
fn group_find_bsearches_the_group_table() {
    let m = tag_index(0x004e_0000);
    for (i, s) in [b"bipd", b"obje", b"scnr", b"unit"].iter().enumerate() {
        assert_eq!(
            tag_group_find(&m, g(s)).unwrap(),
            Some(GROUPS + i as u32 * 12)
        );
    }
    assert_eq!(tag_group_find(&m, g(b"weap")).unwrap(), None);
}

#[test]
fn group_find_without_an_index() {
    let mut m = tag_index(0x004e_0000);
    m.set_u32(TAG_INDEX_GLOBAL, 0).unwrap();
    assert_eq!(tag_group_find(&m, g(b"scnr")).unwrap(), None);
}

#[test]
fn tag_get_checked_matches_group_parent_and_grandparent() {
    let m = tag_index(0x004e_0000);
    let scnr = DatumHandle(0xe174_0000);
    let bipd = DatumHandle(0xe175_0001);
    assert_eq!(
        tag_get_checked(&m, scnr, g(b"scnr")).unwrap(),
        Some(0x1000_0000)
    );
    assert_eq!(
        tag_get_checked(&m, bipd, g(b"bipd")).unwrap(),
        Some(0x1000_2000)
    );
    assert_eq!(
        tag_get_checked(&m, bipd, g(b"unit")).unwrap(),
        Some(0x1000_2000),
        "parent"
    );
    assert_eq!(
        tag_get_checked(&m, bipd, g(b"obje")).unwrap(),
        Some(0x1000_2000),
        "grandparent"
    );
    assert_eq!(
        tag_get_checked(&m, bipd, g(b"scnr")).unwrap(),
        None,
        "mismatch"
    );
    assert_eq!(tag_get_checked(&m, scnr, g(b"obje")).unwrap(), None);
}

#[test]
fn tag_get_checked_rejects_bad_datums() {
    let m = tag_index(0x004e_0000);
    assert_eq!(
        tag_get_checked(&m, DatumHandle(0xe175_0000), g(b"scnr")).unwrap(),
        None,
        "stale salt"
    );
    assert_eq!(
        tag_get_checked(&m, DatumHandle(0xe174_0003), g(b"scnr")).unwrap(),
        None,
        "past count"
    );
    assert_eq!(
        tag_get_checked(&m, DatumHandle(0xe174_8000), g(b"scnr")).unwrap(),
        None,
        "negative slot"
    );
    assert_eq!(
        tag_get_checked(&m, DatumHandle::NONE, GroupTag::NONE).unwrap(),
        None,
        "none"
    );
    // The empty slot's datum is NONE, but NONE's slot is -1.
    assert_eq!(
        tag_get_checked(&m, DatumHandle(0xffff_0002), GroupTag::NONE).unwrap(),
        None
    );
}

#[test]
fn tag_get_checked_none_group_matches_absent_ancestors() {
    let m = tag_index(0x004e_0000);
    // scnr has no parent, so NONE matches its parent field.
    assert_eq!(
        tag_get_checked(&m, DatumHandle(0xe174_0000), GroupTag::NONE).unwrap(),
        Some(0x1000_0000)
    );
    // bipd has a full ancestry: no match.
    assert_eq!(
        tag_get_checked(&m, DatumHandle(0xe175_0001), GroupTag::NONE).unwrap(),
        None
    );
}

#[test]
fn tag_with_unknown_group_reads_address_zero() {
    let mut m = tag_index(0x004e_0000);
    m.set_u32(ENTRIES + 16, g(b"weap").0).unwrap();
    let bipd = DatumHandle(0xe175_0001);
    assert_eq!(
        tag_get_checked(&m, bipd, g(b"weap")),
        Err(MemFault {
            addr: 0,
            len: 4,
            write: false
        })
    );
    // With page 0 mapped, whatever is there is treated as the record.
    let mut m = tag_index(0);
    m.set_u32(ENTRIES + 16, g(b"weap").0).unwrap();
    m.set_u32(4, g(b"item").0).unwrap();
    assert_eq!(
        tag_get_checked(&m, bipd, g(b"item")).unwrap(),
        Some(0x1000_2000)
    );
    assert_eq!(tag_get_checked(&m, bipd, g(b"weap")).unwrap(), None);
}

// ---- locale ---------------------------------------------------------------

const REC: u32 = 0x0020_0000;
const LIDX: u32 = REC + 0x100;
const BLOB: u32 = REC + 0x1000;

fn locale(loaded: bool) -> (FlatMemory, LocaleTable) {
    let mut m = FlatMemory::new(REC, 0x2000);
    let ids = [0x0500_0001u32, 0x0300_0002, 0x0700_0003, 0x0400_0004];
    for (i, id) in ids.iter().enumerate() {
        m.set_u32(LIDX + i as u32 * 8, *id).unwrap();
        m.set_u32(LIDX + i as u32 * 8 + 4, i as u32 * 0x10).unwrap();
    }
    m.set_u32(REC + loc::INDEX, LIDX).unwrap();
    m.set_u32(REC + loc::BLOB, BLOB).unwrap();
    m.set_u32(REC + loc::COUNT, ids.len() as u32).unwrap();
    m.set_u8(REC + loc::LOADED, loaded as u8).unwrap();
    (m, LocaleTable::at(REC))
}

#[test]
fn locale_find_returns_blob_offset_or_empty_string() {
    let (m, t) = locale(true);
    assert_eq!(
        t.find(&m, 0, 4, StringId(0x0700_0003)).unwrap(),
        BLOB + 0x20
    );
    assert_eq!(
        t.find(&m, 0, 4, StringId(0x0900_0009)).unwrap(),
        EMPTY_STRING,
        "missing"
    );
    assert_eq!(
        t.find(&m, 0, 2, StringId(0x0700_0003)).unwrap(),
        EMPTY_STRING,
        "outside range"
    );
    assert_eq!(
        t.find(&m, 2, 1, StringId(0x0700_0003)).unwrap(),
        BLOB + 0x20
    );
    assert_eq!(
        t.find(&m, 0, 0, StringId(0x0500_0001)).unwrap(),
        EMPTY_STRING,
        "empty range"
    );
}

#[test]
fn locale_find_compares_the_whole_packed_id() {
    let (m, t) = locale(true);
    // Same low 24 bits, different length byte.
    assert_eq!(
        t.find(&m, 0, 4, StringId(0x0600_0003)).unwrap(),
        EMPTY_STRING
    );
}

#[test]
fn locale_find_skips_indices_outside_the_table() {
    let (m, t) = locale(true);
    assert_eq!(
        t.find(&m, -3, 5, StringId(0x0500_0001)).unwrap(),
        BLOB,
        "negatives skipped, not read"
    );
    assert_eq!(
        t.find(&m, 3, 100, StringId(0x0400_0004)).unwrap(),
        BLOB + 0x30
    );
    // Entries past the table count are never read, even if the range covers
    // them and memory there matches.
    let mut m = m;
    m.set_u32(LIDX + 4 * 8, 0x0100_0005).unwrap();
    assert_eq!(
        t.find(&m, 0, 10, StringId(0x0100_0005)).unwrap(),
        EMPTY_STRING
    );
}

#[test]
fn locale_contains_checks_loaded_first() {
    let (m, t) = locale(true);
    assert!(t.contains(&m, 0, 4, StringId(0x0300_0002)).unwrap());
    assert!(!t.contains(&m, 0, 4, StringId(0x0300_0009)).unwrap());
    let (m, t) = locale(false);
    assert!(!t.contains(&m, 0, 4, StringId(0x0300_0002)).unwrap());
}

#[test]
fn locale_record_address_in_globals() {
    assert_eq!(
        LocaleTable::in_globals(0x1000, 2).addr,
        0x1000 + 0x188 + 2 * 0x1c
    );
}

// ---- adapters -------------------------------------------------------------

const STACK: u32 = BASE + 0xf000;
const RET: u32 = 0x00c0_ffee;

fn call(addr: u32, m: &mut FlatMemory, mut r: Regs, args: &[u32]) -> Regs {
    let mut sp = STACK;
    for a in args.iter().rev() {
        sp -= 4;
        m.set_u32(sp, *a).unwrap();
    }
    sp -= 4;
    m.set_u32(sp, RET).unwrap();
    r.esp = sp;
    let o = abi::find(addr).unwrap();
    assert_eq!((o.run)(&mut r, m).unwrap(), RET);
    r
}

#[test]
fn adapters_follow_the_register_contracts() {
    let (mut m, a) = array(b"ab", 4, 4);
    let r = call(
        0x16b840,
        &mut m,
        Regs {
            edx: a.addr,
            ecx: 7,
            ..Default::default()
        },
        &[],
    );
    assert_eq!(r.eax, DatumHandle::new(seed(b"ab"), 0).raw());
    assert_eq!(
        (r.edx, r.ecx, r.esp),
        (a.addr, 7, STACK),
        "inputs and scratch left alone"
    );

    let h = r.eax;
    let r = call(
        0x16bae0,
        &mut m,
        Regs {
            eax: h,
            esi: a.addr,
            ..Default::default()
        },
        &[],
    );
    assert_eq!(r.eax, a.header(&m).unwrap().data);
    let r = call(
        0x16bae0,
        &mut m,
        Regs {
            eax: h ^ 0x1_0000,
            esi: a.addr,
            ..Default::default()
        },
        &[],
    );
    assert_eq!(r.eax, 0);

    // data_last_datum: stdcall, one argument, which it overwrites.
    let r = call(0x16bc40, &mut m, Regs::default(), &[a.addr]);
    assert_eq!(r.eax, h);
    assert_eq!(r.esp, STACK, "ret 4");
    assert_eq!(
        m.u32(STACK - 4).unwrap(),
        h,
        "argument slot holds the result"
    );
}

#[test]
fn locale_exists_adapter_sets_only_al() {
    let (mut lm, _) = locale(true);
    let mut m = FlatMemory::new(REC, 0x10000);
    m.bytes[..0x2000].copy_from_slice(&lm.bytes);
    lm.bytes.clear();
    let stack = REC + 0x8000;
    m.set_u32(stack, RET).unwrap();
    m.set_u32(stack + 4, 4).unwrap();
    let mut r = Regs {
        eax: 0x1234_5600,
        esi: REC,
        ecx: 0,
        ebx: 0x0300_0002,
        esp: stack,
        ..Default::default()
    };
    let o = abi::find(0x1a0310).unwrap();
    assert_eq!((o.run)(&mut r, &mut m).unwrap(), RET);
    assert_eq!((r.eax, r.esp), (0x1234_5601, stack + 8));
}

#[test]
fn override_table_is_sorted_and_unique() {
    let addrs: Vec<u32> = abi::OVERRIDES.iter().map(|o| o.addr).collect();
    let mut sorted = addrs.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(addrs, sorted);
}

// ---- review fixtures -------------------------------------------------------

/// `rep stosd` at 0x16b8d3 zeroes the mapped dwords of an element that runs
/// off the end of memory before faulting; the bookkeeping writes that come
/// first have happened too.
#[test]
fn datum_new_fault_keeps_the_rep_stos_prefix() {
    let mut m = FlatMemory::new(0x0020_0000, 0x1000); // mapped to 0x200fff
    let a = DatumArray::at(0x0020_0100);
    let bitmap = 0x0020_0200;
    m.set_i32(a.addr + layout::MAXIMUM_COUNT, 1).unwrap();
    m.set_i32(a.addr + layout::ELEMENT_SIZE, 16).unwrap();
    m.set_u32(a.addr + layout::DATA, 0x0020_0ff8).unwrap();
    m.set_u32(a.addr + layout::BITMAP, bitmap).unwrap();
    m.set_u16(a.addr + layout::NEXT_SALT, 0x8000).unwrap();
    m.write(0x0020_0ff8, &[0xcc; 8]).unwrap();
    let err = a.new_datum(&mut m).unwrap_err();
    assert_eq!(
        err,
        MemFault {
            addr: 0x0020_1000,
            len: 4,
            write: true
        }
    );
    assert_eq!(m.u32(0x0020_0ff8).unwrap(), 0);
    assert_eq!(m.u32(0x0020_0ffc).unwrap(), 0);
    assert_eq!(m.u32(bitmap).unwrap(), 1, "bit set before the fill");
    let h = a.header(&m).unwrap();
    assert_eq!((h.active_count, h.free_scan_start, h.high_water), (1, 1, 1));
    assert_eq!(
        h.next_salt, 0x8000,
        "salt not consumed: the fill faulted first"
    );
}

/// The x86 tests the entry pointer after the datum comparison
/// (`0x122c3d`): an entry at address 0 is "no entry".
#[test]
fn tag_entry_at_address_zero_is_rejected() {
    let mut m = FlatMemory::new(0, 0x0056_0000);
    m.set_u32(TAG_INDEX_GLOBAL, INDEX).unwrap();
    m.set_u32(INDEX + 0x18, 1).unwrap();
    m.set_u32(TAG_ENTRIES_GLOBAL, 0).unwrap();
    let scnr = g(b"scnr");
    m.set_u32(0, scnr.0).unwrap();
    m.set_u32(4, 0x8000_0000).unwrap();
    m.set_u32(8, 0x1234_5678).unwrap();
    // A group table that would accept it, so only the sentinel decides.
    m.set_u32(INDEX, GROUPS).unwrap();
    m.set_u32(INDEX + 4, 1).unwrap();
    m.set_u32(GROUPS, scnr.0).unwrap();
    m.set_u32(GROUPS + 4, u32::MAX).unwrap();
    m.set_u32(GROUPS + 8, u32::MAX).unwrap();
    assert_eq!(
        tag_get_checked(&m, DatumHandle(0x8000_0000), scnr).unwrap(),
        None
    );
    // The same entry at a non-zero address is found.
    m.set_u32(TAG_ENTRIES_GLOBAL, 0x10).unwrap();
    let entry = m.bytes[..12].to_vec();
    m.write(0x10, &entry).unwrap();
    assert_eq!(
        tag_get_checked(&m, DatumHandle(0x8000_0000), scnr).unwrap(),
        Some(0x1234_5678)
    );
}

/// With a negative group count the guest bsearch never terminates unless
/// a probe matches. count -1, key 1, groups[-1].group = 2: every probe
/// lands on groups[-1], compares low, and halving -1 stays -1.
#[test]
fn negative_group_count_bsearch_does_not_terminate() {
    use guest_layout::tags::{Bounded, tag_group_find_bounded};
    let mut m = tag_index(0x004e_0000);
    m.set_i32(INDEX + 4, -1).unwrap();
    m.set_u32(GROUPS - 12, 2).unwrap();
    assert_eq!(
        tag_group_find_bounded(&m, GroupTag(1), 100_000).unwrap(),
        Bounded::OutOfBudget
    );
    // A matching probe ends it, but a hit at index -1 reads as a miss.
    assert_eq!(
        tag_group_find_bounded(&m, GroupTag(2), 100_000).unwrap(),
        Bounded::Done(None)
    );
}

#[test]
fn datum_initialize_adapter_stamps_any_element() {
    let (mut m, a) = array(b"ab", 4, 8);
    let elem = a.header(&m).unwrap().data + 8;
    m.write(elem, &[0xee; 8]).unwrap();
    let r = call(
        0x16ba00,
        &mut m,
        Regs {
            edx: a.addr,
            esi: elem,
            ..Default::default()
        },
        &[],
    );
    assert_eq!((r.edx, r.esi, r.esp), (a.addr, elem, STACK));
    assert_eq!(m.u16(elem).unwrap(), seed(b"ab"));
    assert_eq!(&m.bytes[(elem - BASE) as usize + 2..][..6], &[0; 6]);
    let h = a.header(&m).unwrap();
    assert_eq!(
        (h.next_salt, h.active_count, h.high_water),
        (seed(b"ab") + 1, 0, 0)
    );
}

// ---- argument-read order ----------------------------------------------------

/// Run an adapter whose stack ends at the end of mapped memory: the
/// return address and `mapped_args` are mapped, later arguments are not.
fn call_at_stack_end(
    addr: u32,
    m: &mut FlatMemory,
    mut r: Regs,
    mapped_args: &[u32],
) -> (Regs, Result<u32, MemFault>) {
    let end = m.base + m.bytes.len() as u32;
    let sp = end - 4 - 4 * mapped_args.len() as u32;
    m.set_u32(sp, RET).unwrap();
    for (i, v) in mapped_args.iter().enumerate() {
        m.set_u32(sp + 4 + 4 * i as u32, *v).unwrap();
    }
    r.esp = sp;
    let o = abi::find(addr).unwrap();
    let res = (o.run)(&mut r, m);
    (r, res)
}

fn read_fault(addr: u32, len: u32) -> Result<u32, MemFault> {
    Err(MemFault {
        addr,
        len,
        write: false,
    })
}

/// 0x16b650 clears the header (0x16b65a) before it reads the name argument
/// (0x16b65c), so an unmapped name argument faults with the header zeroed.
#[test]
fn data_initialize_clears_the_header_before_reading_its_arguments() {
    let mut m = FlatMemory::new(BASE, 0x10000);
    let end = BASE + 0x10000;
    m.write(ARRAY, &[0xcc; 0x4c]).unwrap();
    let regs = Regs {
        esi: ARRAY,
        ..Default::default()
    };
    let (_, res) = call_at_stack_end(0x16b650, &mut m, regs, &[]);
    assert_eq!(res, read_fault(end, 4));
    assert_eq!(&m.bytes[0x100..0x14c], &[0; 0x4c]);
}

/// With the name, capacity and element size mapped, the alignment byte
/// read (`mov cl, [esp+20h]`) faults after the name copy and the capacity
/// store, but before the element size store.
#[test]
fn data_initialize_reads_later_arguments_between_its_stores() {
    let mut m = FlatMemory::new(BASE, 0x10000);
    let end = BASE + 0x10000;
    m.write(NAME, b"abc\0").unwrap();
    m.write(ARRAY, &[0xcc; 0x4c]).unwrap();
    let regs = Regs {
        esi: ARRAY,
        ..Default::default()
    };
    let (_, res) = call_at_stack_end(0x16b650, &mut m, regs, &[NAME, 7, 0x10]);
    assert_eq!(res, read_fault(end, 1));
    let h = DatumArray::at(ARRAY).header(&m).unwrap();
    assert_eq!(&h.name[..4], b"abc\0");
    assert_eq!((h.maximum_count, h.element_size), (7, 0));
    assert_eq!(h.flags, flags::BIT0 | flags::BIT1);
    assert_eq!(h.signature, 0);
}

/// 0x16b5f0 reads buffer, then allocator, then name, so with only the
/// buffer argument mapped the fault is at the allocator slot.
#[test]
fn data_initialize_contiguous_reads_the_allocator_before_the_name() {
    let mut m = FlatMemory::new(BASE, 0x10000);
    let end = BASE + 0x10000;
    let regs = Regs {
        ebx: 2,
        edx: 4,
        ..Default::default()
    };
    let (_, res) = call_at_stack_end(0x16b5f0, &mut m, regs, &[ARRAY]);
    assert_eq!(res, read_fault(end + 4, 4));
}

/// 0x1a0310 tests the loaded flag before it reads its count argument, so
/// an unloaded table returns false even when the count is unmapped.
#[test]
fn locale_exists_reads_the_count_only_when_loaded() {
    let (lm, _) = locale(false);
    let mut m = FlatMemory::new(REC, 0x10000);
    m.bytes[..0x2000].copy_from_slice(&lm.bytes);
    let end = REC + 0x10000;
    let regs = Regs {
        eax: 0x1234_56ff,
        esi: REC,
        ebx: 0x0300_0002,
        ..Default::default()
    };
    let (r, res) = call_at_stack_end(0x1a0310, &mut m, regs, &[]);
    assert_eq!(res, Ok(RET));
    assert_eq!((r.eax, r.esp), (0x1234_5600, end + 4));

    m.set_u8(REC + loc::LOADED, 1).unwrap();
    let (_, res) = call_at_stack_end(0x1a0310, &mut m, regs, &[]);
    assert_eq!(res, read_fault(end, 4));
}
