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
const BLOCKS: [u32; 3] = [0x100, 0x200, 0x1d00];
const HEADER: u32 = lockstep::gameplay::BLOCK_HEADER as u32;

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
    // The storage is a chain of blocks, each starting with its size.
    let mut at = POOL_STORAGE;
    for size in BLOCKS {
        m.write(at, &size.to_le_bytes()).unwrap();
        at += size;
    }
    // The object globals past the storage: a sequence number the last game
    // reached, its placement scratch, and a word that is not theirs.
    m.write(POOL_STORAGE + POOL_SIZE + 0x28, &[0x29; 0x58]).unwrap();
    m.write(POOL_STORAGE + POOL_SIZE + 0x10, &1u32.to_le_bytes()).unwrap();
    m
}

#[test]
fn an_empty_object_array_gets_clean_block_bodies_and_keeps_every_block_header() {
    let mut m = pooled(0);
    let bodies: u32 = BLOCKS.iter().map(|b| b - HEADER).sum();
    assert_eq!(clear_object_pool(&mut m).unwrap(), Cleared::Zeroed(bodies as usize + 0x58));
    let mut at = POOL_STORAGE;
    for size in BLOCKS {
        let header = m.bytes(at, HEADER as usize).unwrap();
        assert_eq!(u32::from_le_bytes(header[..4].try_into().unwrap()), size, "the size stays");
        assert!(header[4..].iter().all(|b| *b == 0x5a), "so does the rest of the header");
        let body = m.bytes(at + HEADER, (size - HEADER) as usize).unwrap();
        assert!(body.iter().all(|b| *b == 0), "the body is clean");
        at += size;
    }
    // The pool's own header is as it was. The object globals past the pool
    // are clean from the sequence number on; the words before it are not
    // touched.
    assert_eq!(m.u32(POOL + 0x28).unwrap(), POOL_STORAGE);
    let globals = m.bytes(POOL_STORAGE + POOL_SIZE + 0x28, 0x58).unwrap();
    assert!(globals.iter().all(|b| *b == 0), "sequence number and scratch");
    assert_eq!(m.u32(POOL_STORAGE + POOL_SIZE + 0x10).unwrap(), 1);
}

#[test]
fn a_chain_that_breaks_is_cleared_as_far_as_it_holds() {
    let mut m = pooled(0);
    // The second block's size is garbage: the walk stops after the first.
    m.write(POOL_STORAGE + BLOCKS[0], &0u32.to_le_bytes()).unwrap();
    assert_eq!(
        clear_object_pool(&mut m).unwrap(),
        Cleared::Zeroed((BLOCKS[0] - HEADER) as usize + 0x58)
    );
    assert_eq!(m.u8(POOL_STORAGE + BLOCKS[0] + HEADER).unwrap(), 0x5a, "past the break is left");
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
