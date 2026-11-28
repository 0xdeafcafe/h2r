use super::*;
use guest_layout::player::Player;
use guest_layout::{FlatRam, Mem, MemMut};

const BASE: u32 = 0x0040_0000;
const ARRAY: u32 = 0x0050_0000;
const DATA: u32 = 0x0050_1000;
const BITMAP: u32 = 0x0050_2000;
const GLOBALS: u32 = 0x0050_3000;
const FIRST: u32 = 0x0050_4000;

fn w(ram: &mut FlatRam, at: u32, v: &[u8]) {
    ram.write_bytes(at, v).unwrap();
}

/// An arena with the players array of `salts` (0 is a free slot), the
/// globals block (`globals[0xc..]` the local-user map) and the first-person
/// block, each byte of a live player `slot as u8 + 1` from `+2`.
fn arena(salts: &[u16], next_salt: u16, scan: u32) -> FlatRam {
    let mut ram = FlatRam::new(BASE, 0x20_0000);
    w(&mut ram, guest_layout::global::PLAYERS.base, &ARRAY.to_le_bytes());
    w(&mut ram, guest_layout::global::PLAYER_GLOBALS.base, &GLOBALS.to_le_bytes());
    w(&mut ram, guest_layout::global::FIRST_PERSON.base, &FIRST.to_le_bytes());
    w(&mut ram, ARRAY + 0x20, &16i32.to_le_bytes());
    w(&mut ram, ARRAY + 0x24, &(PLAYER_SIZE as i32).to_le_bytes());
    w(&mut ram, ARRAY + 0x34, &(scan as i32).to_le_bytes());
    w(&mut ram, ARRAY + 0x38, &(salts.len() as i32).to_le_bytes());
    w(&mut ram, ARRAY + 0x3c, &(salts.iter().filter(|s| **s != 0).count() as i32).to_le_bytes());
    w(&mut ram, ARRAY + 0x40, &next_salt.to_le_bytes());
    w(&mut ram, ARRAY + 0x44, &DATA.to_le_bytes());
    w(&mut ram, ARRAY + 0x48, &BITMAP.to_le_bytes());
    let mut bits = 0u32;
    for (slot, &salt) in salts.iter().enumerate() {
        if salt == 0 {
            continue;
        }
        bits |= 1 << slot;
        let at = DATA + slot as u32 * PLAYER_SIZE;
        w(&mut ram, at, &salt.to_le_bytes());
        w(&mut ram, at + 2, &[slot as u8 + 1; RECORD_SIZE]);
    }
    w(&mut ram, BITMAP, &bits.to_le_bytes());
    w(&mut ram, GLOBALS + 0xc, &[0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]);
    w(&mut ram, FIRST + 4, &0xe26f_0001u32.to_le_bytes());
    w(&mut ram, FIRST + 3 * FIRST_PERSON_USER_SIZE as u32 + 0x10, &[7; 16]);
    ram
}

/// `players_initialize`'s array: salts start at `"pl" | 0x8000`, and an
/// import keeps the allocation state with the live slots.
#[test]
fn import_reads_the_array_and_its_allocation_state() {
    let ram = arena(&[0xec70, 0, 0xec72], 0xec73, 1);
    let s = PlayersState::import(&ram).unwrap().unwrap();
    assert_eq!((s.players.len(), s.players.high_water(), s.players.free_scan_start()), (2, 3, 1));
    assert_eq!(s.players.next_salt(), 0xec73);
    let live: Vec<_> = s.iter().map(|(h, r)| (h.raw(), r.bytes[0], r.bytes[RECORD_SIZE - 1])).collect();
    assert_eq!(live, [(0xec70_0000, 1, 1), (0xec72_0002, 3, 3)]);
    assert_eq!((s.globals.addr, s.first_person.addr), (GLOBALS, FIRST));
    assert_eq!(s.globals.bytes[0xc..0x14], [0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]);
}

#[test]
fn no_players_global_is_no_state() {
    let ram = FlatRam::new(BASE, 0x20_0000);
    assert!(PlayersState::import(&ram).unwrap().is_none());
}

#[test]
fn an_array_with_another_element_size_is_corrupt() {
    let mut ram = arena(&[0xec70], 0xec71, 1);
    w(&mut ram, ARRAY + 0x24, &0x200i32.to_le_bytes());
    assert_eq!(
        PlayersState::import(&ram).err(),
        Some(ImportError::Corrupt("element size is not a player's"))
    );
}

/// Scatter writes back what import read, bit for bit, and clears a freed
/// slot's salt and bitmap bit.
#[test]
fn scatter_writes_the_state_back_bit_for_bit() {
    let ram = arena(&[0xec70, 0, 0xec72], 0xec73, 1);
    let mut s = PlayersState::import(&ram).unwrap().unwrap();
    // Into an arena that has the arrays' storage but none of the state.
    let mut out = arena(&[], 0xec70, 0);
    s.scatter(&mut out).unwrap();
    assert_eq!(out, ram);
    // A delete clears the salt, the bit and lowers the high water.
    let gone = s.players.handle_at(2).unwrap();
    s.delete(gone).unwrap();
    s.scatter(&mut out).unwrap();
    let mut salt = [0u8; 2];
    out.read_bytes(DATA + 2 * PLAYER_SIZE, &mut salt).unwrap();
    assert_eq!(salt, [0, 0]);
    let mut bits = [0u8; 4];
    out.read_bytes(BITMAP, &mut bits).unwrap();
    assert_eq!(u32::from_le_bytes(bits), 1);
    let mut high = [0u8; 4];
    out.read_bytes(ARRAY + 0x38, &mut high).unwrap();
    assert_eq!(i32::from_le_bytes(high), 1);
}

/// Allocation is `datum_new`: first free slot from the scan start, salts
/// from the name seed, capacity 16.
#[test]
fn allocation_follows_datum_new() {
    let mut s = PlayersState::default();
    let a = s.alloc(PlayerRecord::default()).unwrap();
    let b = s.alloc(PlayerRecord::default()).unwrap();
    assert_eq!((a.raw(), b.raw()), (0xec70_0000, 0xec71_0001));
    s.delete(a).unwrap();
    assert_eq!(s.alloc(PlayerRecord::default()).unwrap().raw(), 0xec72_0000);
    for _ in 2..16 {
        s.alloc(PlayerRecord::default()).unwrap();
    }
    assert!(s.alloc(PlayerRecord::default()).is_none());
}

#[test]
fn encode_decode_round_trips_with_free_slots_in_place() {
    let ram = arena(&[0xec70, 0, 0xec72], 0xec73, 1);
    let s = PlayersState::import(&ram).unwrap().unwrap();
    let bytes = s.encode();
    let back = PlayersState::decode(PlayersState::VERSION, &bytes).unwrap();
    assert_eq!(back.encode(), bytes);
    assert_eq!(back.section_hash(), s.section_hash());
    assert_eq!(back.players.next_salt(), 0xec73);
    let mut after = back;
    assert_eq!(after.alloc(PlayerRecord::default()).unwrap().index(), 1);
}

#[test]
fn a_truncated_or_trailing_section_is_refused() {
    let s = PlayersState::default();
    let bytes = s.encode();
    assert!(PlayersState::decode(PlayersState::VERSION, &bytes[..bytes.len() - 1]).is_err());
    let mut more = bytes.clone();
    more.push(0);
    assert!(PlayersState::decode(PlayersState::VERSION, &more).is_err());
    assert!(PlayersState::decode(9, &bytes).is_err());
}

/// The typed views read the pool as they read the arena.
#[test]
fn players_mem_reads_as_the_arena_does() {
    let ram = arena(&[0xec70, 0, 0xec72], 0xec73, 1);
    let s = PlayersState::import(&ram).unwrap().unwrap();
    let mem = PlayersMem::new(&s);
    let array = DatumArray::at(guest_layout::global::PLAYERS.pointer(&mem).unwrap().unwrap());
    assert_eq!(array.high_water(&mem).unwrap(), 3);
    assert_eq!(array.active_count(&mem).unwrap(), 2);
    let p = Player::at(array.data(&mem).unwrap() + 2 * PLAYER_SIZE);
    assert_eq!(p.salt(&mem).unwrap(), 0xec72);
    assert_eq!(p.listed(&mem).unwrap(), true);
    let free = Player::at(array.data(&mem).unwrap() + PLAYER_SIZE);
    assert_eq!(free.listed(&mem).unwrap(), false);
    let fp = guest_layout::global::FIRST_PERSON.pointer(&mem).unwrap().unwrap();
    let mut unit = [0u8; 4];
    mem.read_bytes(fp + 4, &mut unit).unwrap();
    assert_eq!(u32::from_le_bytes(unit), 0xe26f_0001);
    let mut nothing = [0u8; 4];
    assert!(mem.read_bytes(0x1234, &mut nothing).is_err());
}
