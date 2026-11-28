use super::*;
use guest_layout::testing::FakeGuest;

const CACHE: u32 = 0x0060_0000;
const CACHE_DATA: u32 = 0x0060_1000;
const CACHE_BITMAP: u32 = 0x0060_0800;
const LIGHT_HEADER: u32 = 0x0061_0000;
const LIGHT_DATA: u32 = 0x0061_1000;
const OBJECT_HEADER: u32 = 0x0062_0000;
const OBJECT_DATA: u32 = 0x0062_1000;
const BODY: u32 = 0x0063_0000;
/// A tick-owned light byte beside the render fields.
const LIGHT_TICK_FIELD: u32 = LIGHT_DATA + 0x4c;

fn header(m: &mut FakeGuest, at: u32, size: u32, count: u32, high_water: u32, data: u32, bitmap: u32) {
    m.map(at, layout::HEADER_SIZE)
        .put_u32(at + layout::SIGNATURE, guest_layout::datum::SIGNATURE)
        .put_u32(at + layout::ELEMENT_SIZE, size)
        .put_u32(at + layout::MAXIMUM_COUNT, count)
        .put_u32(at + layout::HIGH_WATER, high_water)
        .put_u32(at + layout::DATA, data)
        .put_u32(at + layout::BITMAP, bitmap)
        .map(data, size * count);
}

/// Two cache records, two lights and one live object (with a dead one).
fn guest() -> FakeGuest {
    let mut m = FakeGuest::new();
    m.put_u32(RENDER_STATES, CACHE).put_u32(LIGHTS, LIGHT_HEADER).put_u32(OBJECTS, OBJECT_HEADER);
    header(&mut m, CACHE, RENDER_STATE_SIZE, 2, 0, CACHE_DATA, CACHE_BITMAP);
    m.map(CACHE_BITMAP, 4);
    header(&mut m, LIGHT_HEADER, LIGHT_SIZE, 2, 1, LIGHT_DATA, 0);
    header(&mut m, OBJECT_HEADER, OBJECT_HEADER_SIZE, 4, 2, OBJECT_DATA, 0);
    m.put_u16(OBJECT_DATA, 0x8001).put_u32(OBJECT_DATA + OBJECT_BODY, BODY);
    m.put_u32(BODY + OBJECT_RENDER_STATE, u32::MAX);
    m
}

/// What the frame step writes: a cache record allocated and its handle
/// stored, a light's render fields, a bitmap bit.
fn render(m: &mut FakeGuest, n: u8) {
    m.put_u32(BODY + OBJECT_RENDER_STATE, 0xe000_0001)
        .put_u32(CACHE_DATA + RENDER_STATE_SIZE + 4, 0x8001_0000)
        .put_u32(CACHE_BITMAP, 0b10)
        .put_u32(CACHE + layout::ACTIVE_COUNT, 1)
        .put(LIGHT_DATA + 0xf8, &[n; 0x18])
        .put(LIGHT_DATA + 0x84, &[n; 0x18]);
}

fn snapshot(m: &FakeGuest) -> Vec<u8> {
    let mut out = Vec::new();
    for (addr, len) in [
        (CACHE, layout::HEADER_SIZE),
        (CACHE_BITMAP, 4),
        (CACHE_DATA, 2 * RENDER_STATE_SIZE),
        (LIGHT_DATA, 2 * LIGHT_SIZE),
        (BODY, 0x100),
    ] {
        let mut b = vec![0; len as usize];
        m.read(addr, &mut b).unwrap();
        out.extend(b);
    }
    out
}

#[test]
fn the_fields_are_the_cache_the_light_render_fields_and_each_live_objects_handle() {
    let f = fields(&guest());
    assert!(f.contains(&(CACHE, layout::HEADER_SIZE)));
    assert!(f.contains(&(CACHE_BITMAP, 4)));
    assert!(f.contains(&(CACHE_DATA + RENDER_STATE_SIZE, RENDER_STATE_SIZE)));
    for light in [LIGHT_DATA, LIGHT_DATA + LIGHT_SIZE] {
        for (off, len) in LIGHT_FIELDS {
            assert!(f.contains(&(light + off, len)), "{light:#x}+{off:#x}");
        }
    }
    assert!(f.contains(&(BODY + OBJECT_RENDER_STATE, 4)));
    assert_eq!(f.len(), 2 + 2 + 2 * LIGHT_FIELDS.len() + 1, "the dead object has no field: {f:x?}");
    assert!(
        !f.iter().any(|&(a, l)| (a..a + l).contains(&LIGHT_TICK_FIELD)),
        "the tick's light fields stay the tick's"
    );
}

#[test]
fn no_arrays_no_fields() {
    let mut m = FakeGuest::new();
    m.put_u32(RENDER_STATES, 0).put_u32(LIGHTS, 0).put_u32(OBJECTS, 0);
    assert!(fields(&m).is_empty());
    let mut m = guest();
    m.put_u32(CACHE + layout::ELEMENT_SIZE, 0x80);
    assert!(!fields(&m).iter().any(|&(a, _)| a == CACHE), "not the cache's record size");
}

/// The frame guard's rule for the scratch: a frame that writes it leaves
/// the arena's bytes as they were, whatever it drew.
#[test]
fn a_frame_leaves_the_arena_as_it_found_it() {
    let mut m = guest();
    let mut store = Store::default();
    for n in 1..5 {
        let before = snapshot(&m);
        store.enter(&mut m);
        render(&mut m, n);
        store.leave(&mut m);
        assert_eq!(snapshot(&m), before, "frame {n}");
    }
    assert!(!store.is_empty());
}

/// The next frame continues from the render's copy: the setup finds the
/// record it allocated last frame, as it does when the bytes stay in the
/// arena.
#[test]
fn the_next_frame_starts_from_the_renders_copy() {
    let mut m = guest();
    let mut store = Store::default();
    store.enter(&mut m);
    render(&mut m, 7);
    store.leave(&mut m);
    assert_eq!(m.u32(BODY + OBJECT_RENDER_STATE).unwrap(), u32::MAX);

    store.enter(&mut m);
    assert_eq!(m.u32(BODY + OBJECT_RENDER_STATE).unwrap(), 0xe000_0001);
    assert_eq!(m.u32(CACHE_DATA + RENDER_STATE_SIZE + 4).unwrap(), 0x8001_0000);
    assert_eq!(m.u32(CACHE_BITMAP).unwrap(), 0b10);
    assert_eq!(m.u8(LIGHT_DATA + 0xf8).unwrap(), 7);
    store.enter(&mut m); // entered already: nothing
    store.leave(&mut m);
    store.leave(&mut m); // the second leave: nothing
    assert_eq!(m.u32(BODY + OBJECT_RENDER_STATE).unwrap(), u32::MAX);
}

/// A field the tick wrote between frames (a reused object's `+0xcc`, the
/// tick's own `0x0003ddd0` call, a light re-made) starts from the tick's
/// bytes; the fields it did not write keep the render's.
#[test]
fn a_field_the_tick_wrote_starts_from_the_ticks_bytes() {
    let mut m = guest();
    let mut store = Store::default();
    store.enter(&mut m);
    render(&mut m, 3);
    store.leave(&mut m);

    m.put_u32(BODY + OBJECT_RENDER_STATE, 0xe001_0000); // the tick's own record
    m.put(LIGHT_DATA + 0x84, &[9; 4]); // the tick moved the light
    store.enter(&mut m);
    assert_eq!(m.u32(BODY + OBJECT_RENDER_STATE).unwrap(), 0xe001_0000);
    assert_eq!(m.u8(LIGHT_DATA + 0x84).unwrap(), 9);
    assert_eq!(m.u8(LIGHT_DATA + 0x88).unwrap(), 0, "the whole field is the tick's");
    assert_eq!(m.u8(LIGHT_DATA + 0xf8).unwrap(), 3, "an untouched field keeps the render's");
    store.leave(&mut m);
    assert_eq!(m.u32(BODY + OBJECT_RENDER_STATE).unwrap(), 0xe001_0000);
}

/// The tick never reads a byte a frame wrote: whatever the frame did, the
/// tick-owned byte beside the render fields and every field read between
/// frames hold the tick's values, the same with no frame drawn.
#[test]
fn the_tick_sees_the_same_bytes_with_the_frame_skipped_or_drawn() {
    let tick = |m: &mut FakeGuest, t: u8| {
        m.put_u8(LIGHT_TICK_FIELD, t);
    };
    let mut skipped = guest();
    let mut drawn = guest();
    let mut store = Store::default();
    for t in 0..6 {
        tick(&mut skipped, t);
        tick(&mut drawn, t);
        store.enter(&mut drawn);
        render(&mut drawn, t.wrapping_mul(17));
        store.leave(&mut drawn);
        assert_eq!(snapshot(&drawn), snapshot(&skipped), "tick {t}");
    }
}

#[test]
fn a_deleted_objects_field_is_dropped_and_a_games_end_drops_all() {
    let mut m = guest();
    let mut store = Store::default();
    store.enter(&mut m);
    render(&mut m, 1);
    store.leave(&mut m);
    let held = store.len();
    m.put_u16(OBJECT_DATA, 0); // the object is deleted
    store.enter(&mut m);
    store.leave(&mut m);
    assert_eq!(store.len(), held - 1);
    store.clear();
    assert!(store.is_empty());
}
