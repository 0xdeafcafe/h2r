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

const OBJECTS: u32 = ARENA + 0x40;
const POOL: u32 = ARENA + 0x1000;
const POOL_STORAGE: u32 = POOL + 0x40;
const POOL_SIZE: u32 = 0x2000;

/// An arena with the `object` array, the object memory pool after it (its
/// storage full of what an earlier game left) and one array after the pool.
fn pooled(live: i32) -> FlatMemory {
    let mut m = FlatMemory::new(0x004e_0000, 0x0050_0000);
    m.write(POINTER, &ARENA.to_le_bytes()).unwrap();
    let sig = engine::datum::SIGNATURE.to_le_bytes();
    for (at, name) in [(OBJECTS, "object"), (ARENA + 0x4000, "actor")] {
        m.write(at, name.as_bytes()).unwrap();
        m.write(at + layout::SIGNATURE, &sig).unwrap();
    }
    m.write(OBJECTS + layout::ACTIVE_COUNT, &live.to_le_bytes()).unwrap();
    m.write(POOL, b"loopobjects").unwrap();
    m.write(POOL + 0x28, &POOL_STORAGE.to_le_bytes()).unwrap();
    m.write(POOL + 0x2c, &POOL_SIZE.to_le_bytes()).unwrap();
    m.write(POOL_STORAGE, &vec![0x5a; POOL_SIZE as usize]).unwrap();
    m
}

#[test]
fn an_empty_object_array_gets_a_clean_pool_but_for_the_first_block_header() {
    let mut m = pooled(0);
    let keep = lockstep::gameplay::POOL_KEEP as u32;
    assert_eq!(clear_object_pool(&mut m).unwrap(), Cleared::Zeroed((POOL_SIZE - keep) as usize));
    let kept = m.bytes(POOL_STORAGE, keep as usize).unwrap();
    assert!(kept.iter().all(|b| *b == 0x5a), "the first block header stays");
    let cleared = m.bytes(POOL_STORAGE + keep, (POOL_SIZE - keep) as usize).unwrap();
    assert!(cleared.iter().all(|b| *b == 0));
    // The header, and what is after the pool, are as they were.
    assert_eq!(m.u32(POOL + 0x28).unwrap(), POOL_STORAGE);
    assert_eq!(m.u8(POOL_STORAGE + POOL_SIZE).unwrap(), 0);
}

#[test]
fn a_live_object_leaves_the_pool_alone() {
    let mut m = pooled(3);
    let before = m.bytes.clone();
    assert_eq!(clear_object_pool(&mut m).unwrap(), Cleared::Live(3));
    assert_eq!(m.bytes, before);
}

#[test]
fn an_arena_without_the_pool_is_left_alone() {
    let mut m = guest();
    let before = m.bytes.clone();
    assert_eq!(clear_object_pool(&mut m).unwrap(), Cleared::NoPool);
    assert_eq!(m.bytes, before);
}
