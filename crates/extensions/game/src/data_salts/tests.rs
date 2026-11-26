use super::*;
use engine::FlatMemory;
use engine::datum::layout;

const POINTER: u32 = 0x004e_6080;
const ARENA: u32 = 0x005e_0000;

/// A guest with an arena of three data arrays whose counters have moved on.
fn guest() -> FlatMemory {
    let mut m = FlatMemory::new(0x004e_0000, 0x0050_0000);
    m.write(POINTER, &ARENA.to_le_bytes()).unwrap();
    for (i, (name, salt)) in [("objects", 0x8321u16), ("actor", 0xa000), ("hs thread", 0x8010)]
        .into_iter()
        .enumerate()
    {
        let at = ARENA + 0x40 + i as u32 * 0x400;
        m.write(at, name.as_bytes()).unwrap();
        m.write(at + layout::SIGNATURE, &engine::datum::SIGNATURE.to_le_bytes()).unwrap();
        m.write(at + layout::NEXT_SALT, &salt.to_le_bytes()).unwrap();
    }
    m
}

#[test]
fn every_array_is_seeded_as_data_delete_all_seeds_it() {
    let mut m = guest();
    assert_eq!(reseed(&mut m).unwrap(), 3);
    for (i, name) in [b"ob", b"ac", b"hs"].into_iter().enumerate() {
        let at = ARENA + 0x40 + i as u32 * 0x400 + layout::NEXT_SALT;
        assert_eq!(m.u16(at).unwrap(), 0x8000 | u16::from_le_bytes(*name));
    }
    // Seeded again, nothing changes.
    let once = m.bytes.clone();
    reseed(&mut m).unwrap();
    assert_eq!(m.bytes, once);
}

#[test]
fn no_arena_is_nothing_to_seed() {
    let mut m = FlatMemory::new(0x004e_0000, 0x0050_0000);
    assert_eq!(reseed(&mut m).unwrap(), 0);
}
