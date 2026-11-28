use super::*;
use guest_layout::datum::SIGNATURE;
use guest_layout::entry::{self, EntryFuture, Regs};
use crate::sim_sound::clock::TICK_COUNT_IMPORT;
use guest_layout::testing::FakeGuest;

const HEAP: u32 = 0x0080_0000;
const TICKS: u32 = 0x0041_2000;
const ALLOCATOR: u32 = 0x0046_9000;

/// A guest with game state one byte in (so the first allocation pads), the
/// array names, the classes (channels 3, 2, 1; five rows) and a tick count.
fn guest() -> FakeGuest {
    let mut g = FakeGuest::new();
    g.map(HEAP, 0x4_0000);
    g.put_u32(GAME_STATE_BASE, HEAP).put_u32(GAME_STATE_USED, 1).put_u32(GAME_STATE_CRC, 0);
    g.put_u32(DATUM_ALLOCATOR, ALLOCATOR).put_f32(AMBIENCE_GAIN, 1.0);
    for (a, name) in [(COMPANIONS.name, "companions"), (LOOPS.name, "looping sounds"), (STARTS.name, "starts"), (CLASS_ROWS_NAME, "class rows")] {
        g.put(a, name.as_bytes()).put_u8(a + name.len() as u32, 0);
    }
    g.put_u16(CLASSES + 6, 3).put_u16(CLASSES + 8, 2).put_u16(CLASSES + 0xa, 1).put_u16(CLASSES + 0xc, 5);
    g.put_u32(TICK_COUNT_IMPORT, TICKS).put_u32(TICKS, 0x1000);
    for global in [REFCOUNT, points::SOUND_GLOBALS.base, CLASS_ROWS, LOOPS.global, STARTS.global, COMPANIONS.global, COMPANION_DEFAULT] {
        g.put_u32(global, 0);
    }
    // Stale words the create must clear.
    g.put_u32(points::SOUND_INSTANCES.base, 0x0070_0000).put_u32(points::SOUND_CHANNELS.base, 0x0071_0000);
    g
}

fn run_create(g: FakeGuest, driver_up: bool) -> entry::testing::Run {
    let f = |cx: Cx| -> EntryFuture { Box::pin(async move { create(&cx).await.map(|n| entry::Ret::new(n as u32, 0)) }) };
    entry::testing::run(&f, g, Regs::default(), move |c, _| Regs { eax: if c.target == DRIVER_CREATE { u32::from(driver_up) } else { 0 }, ..Regs::default() })
}

fn run_dispose(g: FakeGuest) -> entry::testing::Run {
    let f = |cx: Cx| -> EntryFuture { Box::pin(async move { dispose(&cx).await.map(|n| entry::Ret::new(n as u32, 0)) }) };
    entry::testing::run(&f, g, Regs::default(), |_, _| Regs::default())
}

fn u32_at(g: &FakeGuest, a: u32) -> u32 {
    g.u32(a).unwrap()
}

#[test]
fn the_crc_register_is_the_reflected_crc32() {
    assert_eq!(!crc_update(!0, b"123456789"), 0xcbf4_3926);
}

#[test]
fn a_game_state_allocation_aligns_and_feeds_its_size_to_the_crc() {
    let mut g = guest();
    let at = game_state_alloc(&mut g, 0x20c).unwrap();
    assert_eq!(at, HEAP + 4);
    // Three bytes of padding and the size: taken 0x20f, used 1 + 0x20f.
    assert_eq!(u32_at(&g, GAME_STATE_USED), 0x210);
    assert_eq!(u32_at(&g, GAME_STATE_CRC), crc_update(0, &0x20fu32.to_le_bytes()));
    assert_eq!(game_state_alloc(&mut g, 8).unwrap(), HEAP + 0x210);
}

#[test]
fn data_new_makes_a_valid_empty_contiguous_array() {
    let mut g = guest();
    g.put_u32(GAME_STATE_USED, 0);
    let at = data_new(&mut g, LOOPS).unwrap();
    assert_eq!(at, HEAP);
    assert_eq!(u32_at(&g, LOOPS.global), HEAP);
    assert_eq!(u32_at(&g, at + layout::MAXIMUM_COUNT), 0x80);
    assert_eq!(u32_at(&g, at + layout::ELEMENT_SIZE), 0xd4);
    assert_eq!(u32_at(&g, at + layout::SIGNATURE), SIGNATURE);
    assert_eq!(u32_at(&g, at + layout::ALLOCATOR), ALLOCATOR);
    assert_eq!(u32_at(&g, at + layout::DATA), at + layout::HEADER_SIZE);
    assert_eq!(g.u8(at + layout::VALID), Ok(1));
    assert_eq!(u32_at(&g, at + layout::ACTIVE_COUNT), 0);
    assert_eq!(u32_at(&g, GAME_STATE_USED), 0x80 * 0xd4 + 16 + layout::HEADER_SIZE);
}

#[test]
fn the_first_create_builds_the_globals_and_arrays_and_leaves_the_pool_to_the_table() {
    let r = run_create(guest(), true);
    assert_eq!(r.result, Ok(entry::Ret::new(2, 0)));
    let targets: Vec<u32> = r.calls.iter().map(|c| c.target).collect();
    assert_eq!(targets, [CACHE_CREATE, DRIVER_CREATE]);
    assert_eq!(r.calls[1].args, [CLASSES]);
    let g = &r.guest;
    assert_eq!(u32_at(g, REFCOUNT), 1);
    assert_eq!(u32_at(g, points::SOUND_INSTANCES.base), 0);
    assert_eq!(u32_at(g, points::SOUND_CHANNELS.base), 0);
    let globals = u32_at(g, points::SOUND_GLOBALS.base);
    assert_eq!(globals, HEAP + 4);
    assert_eq!(g.u8(globals + 0x79), Ok(1), "sound on");
    assert_eq!(g.u8(globals + 0x78), Ok(1), "ready");
    assert_eq!(g.u16(globals + 0x208), Ok(6), "the three classes' channels");
    assert_eq!(g.u16(globals + 0x20a), Ok(0x33));
    assert_eq!(u32_at(g, globals + 0x1b0), u32::MAX);
    assert_eq!(u32_at(g, globals + 0x1b8), 1.0f32.to_bits());
    assert_eq!(u32_at(g, globals + 0x1cc), u32::MAX);
    assert_eq!(u32_at(g, globals + 0x1d4), 0);
    for (global, count, size) in [(COMPANIONS.global, 0x40, 0x1c), (LOOPS.global, 0x80, 0xd4), (STARTS.global, 0x10, 0x14), (CLASS_ROWS, 5, 0x6c)] {
        let at = u32_at(g, global);
        assert!(at > globals, "{global:#x} in game state");
        assert_eq!(u32_at(g, at + layout::MAXIMUM_COUNT), count, "{global:#x}");
        assert_eq!(u32_at(g, at + layout::ELEMENT_SIZE), size, "{global:#x}");
        assert_eq!(g.u8(at + layout::VALID), Ok(1), "{global:#x}");
    }
    let record = u32_at(g, COMPANION_DEFAULT);
    assert_eq!(u32_at(g, record + 4), u32::MAX);
    assert_eq!(g.u8(record + 2), Ok(0x80), "not muted");
    assert_eq!(g.u8(record + 3), Ok(0x80));
    assert_eq!(u32_at(g, record + 0xc), 0);
}

#[test]
fn a_driver_that_does_not_come_up_leaves_the_manager_not_ready() {
    let r = run_create(guest(), false);
    assert_eq!(r.calls.len(), 2);
    let globals = u32_at(&r.guest, points::SOUND_GLOBALS.base);
    assert_eq!(r.guest.u8(globals + 0x78), Ok(0));
    assert_eq!(r.guest.u16(globals + 0x208), Ok(0));
    assert_eq!(u32_at(&r.guest, REFCOUNT), 1, "counted all the same");
}

#[test]
fn a_second_create_only_counts() {
    let mut g = guest();
    g.put_u32(REFCOUNT, 1);
    let r = run_create(g, true);
    assert!(r.calls.is_empty());
    assert_eq!(u32_at(&r.guest, REFCOUNT), 2);
    assert_eq!(u32_at(&r.guest, points::SOUND_GLOBALS.base), 0);
    assert_eq!(u32_at(&r.guest, GAME_STATE_USED), 1, "nothing allocated");
}

#[test]
fn the_last_dispose_releases_the_guest_pieces_and_clears_every_global() {
    let created = run_create(guest(), true).guest;
    let rows = u32_at(&created, CLASS_ROWS);
    let r = run_dispose(created);
    let targets: Vec<u32> = r.calls.iter().map(|c| c.target).collect();
    assert_eq!(targets, [CACHE_DISPOSE, DRIVER_DISPOSE]);
    let g = &r.guest;
    assert_eq!(u32_at(g, REFCOUNT), 0);
    for global in [points::SOUND_GLOBALS.base, CLASS_ROWS, LOOPS.global, STARTS.global, COMPANIONS.global, COMPANION_DEFAULT, points::SOUND_INSTANCES.base, points::SOUND_CHANNELS.base] {
        assert_eq!(u32_at(g, global), 0, "{global:#x}");
    }
    assert_eq!(g.u8(rows + layout::VALID), Ok(0), "the class rows marked invalid");
}

#[test]
fn a_dispose_that_is_not_the_last_only_counts() {
    let mut g = run_create(guest(), true).guest;
    g.put_u32(REFCOUNT, 2);
    let r = run_dispose(g);
    assert!(r.calls.is_empty());
    assert_eq!(u32_at(&r.guest, REFCOUNT), 1);
    assert_ne!(u32_at(&r.guest, points::SOUND_GLOBALS.base), 0);
}
