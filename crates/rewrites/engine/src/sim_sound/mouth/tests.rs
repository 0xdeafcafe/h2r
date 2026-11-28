use super::*;
use crate::points::OBJECTS;
use crate::testing::FakeGuest;

const ARRAY: u32 = 0x0060_0000;
const HEADERS: u32 = 0x0060_1000;
const OBJECT: u32 = 0x0060_2000;
const ENTRIES: u32 = 0x0062_0000;
const SOUND_DEF: u32 = 0x0063_0000;
const UNIT_DEF: u32 = 0x0063_1000;
const CLASS_DEF: u32 = 0x0063_2000;
const CLASSES: u32 = 0x0063_3000;
const GLOBALS: u32 = 0x0063_4000;
const DIALOGUE_AT: u32 = 0x0063_5000;
const G: u32 = 0x0064_0000;
const RANGES: u32 = 0x0064_1000;
const PERMS: u32 = 0x0064_2000;
const EXTRAS: u32 = 0x0064_3000;
const RUNTIME: u32 = 0x0064_4000;
const DESCRIPTORS: u32 = 0x0064_5000;
const DATA: u32 = 0x0064_6000;
const CACHE_AT: u32 = 0x0065_0000;
const SLOT_POOL: u32 = 0x0065_1000;
const SLOTS: u32 = 0x0065_2000;
const REQUEST_POOL: u32 = 0x0065_3000;
const REQUEST_ENTRIES: u32 = 0x0065_4000;

const UNIT: u32 = 0x0007_0002;
/// Tag 3 the sound, 5 the unit, 6 the class list.
const TAG: u32 = 0xe100_0003;
const STATE: u32 = 0x100;
/// The sound's first pitch range is row 4; its range 1 (row 5) starts at
/// permutation 10 and holds 3.
const RANGE: u32 = 1;
const FIRST_PERM: u32 = 10;
const EXTRA: u32 = EXTRAS + 2 * 0x2c;
const RESOURCE: u32 = EXTRA + 8;
const EPOCH: u32 = 77;

fn record(index: u32) -> u32 {
    PERMS + (FIRST_PERM + index) * 16
}

/// A unit speaking a lip-synced sound whose extra info is resident. Its
/// permutation 1 names descriptor 2: a mouth curve of 4 ticks
/// (0, 51, 102, 255) and no facial sequence.
fn guest() -> FakeGuest {
    let mut g = FakeGuest::new();
    g.put_u32(OBJECTS.base, ARRAY)
        .put_u32(ARRAY + 0x38, 4)
        .put_u32(ARRAY + 0x24, 12)
        .put_u32(ARRAY + 0x44, HEADERS);
    let h = HEADERS + 2 * 12;
    g.put_u16(h, 7).put_u8(h + 3, 0).put_u32(h + 8, OBJECT);
    g.put_u32(OBJECT, 5)
        .put_u16(OBJECT + 0x33e, STATE as u16)
        .put_f32(OBJECT + APERTURE, 0.0);
    g.put_u32(DIALOGUE, DIALOGUE_AT)
        .put_u8(DIALOGUE_AT + DIALOGUE_ON, 1);
    g.put_u32(TAG_ENTRIES.base, ENTRIES);
    g.put_u32(ENTRIES + 3 * 16 + 8, SOUND_DEF)
        .put_u32(ENTRIES + 5 * 16 + 8, UNIT_DEF)
        .put_u32(ENTRIES + 6 * 16 + 8, CLASS_DEF);
    g.map(UNIT_DEF, 0x200);
    // The sound: class 3, first range 4, extra info 2.
    g.put_u8(SOUND_DEF + 1, 0)
        .put_u8(SOUND_DEF + 2, 3)
        .put_u16(SOUND_DEF + 8, 4)
        .put_u16(SOUND_DEF + 0xe, 2);
    g.put_u32(GLOBALS_TAG.base, GLOBALS)
        .put_u32(GLOBALS + 0xc0, 1)
        .put_u32(GLOBALS + 0xc4, GLOBALS + 0x100)
        .put_u32(GLOBALS + 0x104, 6);
    g.put_u32(CLASS_DEF + 4, CLASSES)
        .put_u8(CLASSES + 3 * 0x5c + 8, 2);
    g.put_u32(GESTALT, G)
        .put_u32(G + 0x24, RANGES)
        .put_u32(G + 0x2c, PERMS)
        .put_u32(G + 0x54, EXTRAS);
    g.put_u16(RANGES + 5 * 12 + 8, FIRST_PERM as u16)
        .put_u16(RANGES + 5 * 12 + 0xa, 3);
    g.map(PERMS, 0x200).put_u8(record(1) + 5, 2);
    g.put_u32(EXTRA, 1).put_u32(EXTRA + 4, RUNTIME);
    g.put_u32(RUNTIME + 4, DATA)
        .put_u32(RUNTIME + 0xc, DESCRIPTORS);
    g.put_u32(DESCRIPTORS + 2 * 16, 0x10)
        .put_u32(DESCRIPTORS + 2 * 16 + 4, 4)
        .put_u32(DESCRIPTORS + 2 * 16 + 0xc, 0);
    g.put(DATA + 0x10, &[0, 51, 102, 255]);
    // The resource: loaded in cache slot 1.
    g.put_u8(RESOURCE + 0x1e, 1)
        .put_u32(RESOURCE + 0x20, 0x0009_0001);
    g.put_u32(CACHE, CACHE_AT)
        .put_u32(CACHE_AT + 0x64, SLOT_POOL)
        .put_u32(CACHE_AT + 0x38, EPOCH)
        .put_u32(SLOT_POOL + 0x44, SLOTS);
    g.map(SLOTS, 0x100);
    g.put_u32(REQUESTS, REQUEST_POOL)
        .put_u32(REQUEST_POOL + 0x44, REQUEST_ENTRIES)
        .map(REQUEST_ENTRIES, 0x100);
    g.map(LOAD_PHASE, 0x10).map(STRUCTURE_MISSES, 4);
    g
}

fn call(seconds: f32) -> Call {
    Call::from_args([UNIT, 0xdead, TAG, RANGE, record(1), seconds.to_bits()])
}

fn aperture_now(g: &FakeGuest) -> f32 {
    g.f32(OBJECT + APERTURE).unwrap()
}

#[test]
fn the_mouth_steps_toward_the_tick_aperture_by_at_most_three_tenths() {
    let mut g = guest();
    let byte = |n: u8| f32::from(n) * f32::from_bits(BYTE);
    let up = f32::from_bits(STEP_UP);
    // 1/30 s is tick 1: 51/255, a step within bounds.
    assert_eq!(run(&mut g, &call(1.0 / 30.0)), Ok(None));
    let first = byte(51);
    assert_eq!(aperture_now(&g), first);
    assert_eq!(
        g.u32(SLOTS + 0x18 + 0x14),
        Ok(EPOCH),
        "the resident resource's slot is stamped"
    );
    // Tick 3 is 1.0: the step stops at 0.3.
    run(&mut g, &call(0.1)).unwrap();
    assert_eq!(aperture_now(&g), first + up);
    // Past the curve the last byte holds.
    run(&mut g, &call(9.0)).unwrap();
    assert_eq!(aperture_now(&g), first + up + up);
}

#[test]
fn the_aperture_reads_the_clamped_tick_and_refuses_a_permutation_out_of_range() {
    let g = guest();
    let one = 0x3f80_0000;
    assert_eq!(
        aperture(&g, TAG, RANGE as i16, 1, -4),
        Ok(0),
        "tick 0 is byte 0"
    );
    assert_eq!(aperture(&g, TAG, RANGE as i16, 1, 3), Ok(one));
    assert_eq!(aperture(&g, TAG, RANGE as i16, 1, 400), Ok(one));
    assert_eq!(
        aperture(&g, TAG, RANGE as i16, 3, 1),
        Ok(0),
        "the range holds 3 permutations"
    );
    assert_eq!(aperture(&g, TAG, RANGE as i16, -1, 1), Ok(0));
}

#[test]
fn the_mouth_stops_for_no_unit_no_lip_sync_or_no_extra_info() {
    let mut g = guest();
    assert_eq!(
        gate(
            &mut g,
            &Call {
                unit: NONE,
                ..call(0.1)
            }
        ),
        Ok(None)
    );
    assert_eq!(
        gate(
            &mut g,
            &Call {
                permutation: 0,
                ..call(0.1)
            }
        ),
        Ok(None)
    );
    g.put_u8(HEADERS + 2 * 12 + 3, 2);
    assert_eq!(
        gate(&mut g, &call(0.1)),
        Ok(None),
        "a type-2 object is not a unit"
    );
    let mut g = guest();
    g.put_u8(SOUND_DEF + 1, 2);
    assert_eq!(
        gate(&mut g, &call(0.1)),
        Ok(None),
        "the sound turns lip sync off"
    );
    let mut g = guest();
    g.put_u8(CLASSES + 3 * 0x5c + 8, 1);
    assert_eq!(
        gate(&mut g, &call(0.1)),
        Ok(None),
        "the class has no lip sync"
    );
    let mut g = guest();
    g.put_u16(SOUND_DEF + 0xe, 0xffff);
    assert_eq!(gate(&mut g, &call(0.1)), Ok(None));
    assert_eq!(aperture_now(&g), 0.0);
}

#[test]
fn the_permutation_index_is_the_record_s_place_in_its_range() {
    let mut g = guest();
    let p = gate(&mut g, &call(0.1)).unwrap().unwrap();
    assert_eq!(
        (p.index, p.extra, p.residency),
        (1, EXTRA, Residency::Resident)
    );
    assert_eq!(p.resource(), RESOURCE);
}

#[test]
fn an_unloaded_resource_without_a_request_goes_to_the_guest_cache() {
    let mut g = guest();
    g.put_u8(RESOURCE + 0x1e, 0)
        .put_u32(RESOURCE + 0x20, NONE)
        .put_u32(RESOURCE + 8, 0x100);
    let p = run(&mut g, &call(0.1)).unwrap().expect("pending");
    assert_eq!(p.residency, Residency::Guest);
    assert_eq!(g.u32(SLOTS + 0x18 + 0x14), Ok(0), "nothing written");
    // The guest answers not resident; the element's in-tag count (1)
    // still lets the mouth run.
    finish(&mut g, &call(0.1), &p, false).unwrap();
    assert_eq!(aperture_now(&g), 0.3);
    g.put_u32(EXTRA, 0);
    finish(&mut g, &call(0.1), &p, false).unwrap();
    assert_eq!(aperture_now(&g), 0.3, "no in-tag data: nothing");
}

#[test]
fn a_queued_request_is_stamped_and_absent_until_done() {
    let mut g = guest();
    g.put_u8(RESOURCE + 0x1e, 0).put_u32(RESOURCE + 8, 0x100);
    g.put_u8(REQUEST_ENTRIES + 12 + 3, 2);
    assert_eq!(residency(&mut g, RESOURCE), Ok(Residency::Absent));
    assert_eq!(g.u32(SLOTS + 0x18 + 0x14), Ok(EPOCH));
    // A lower priority to raise, a finished load, a blocking phase: guest.
    g.put_u8(REQUEST_ENTRIES + 12 + 3, 1);
    assert_eq!(residency(&mut g, RESOURCE), Ok(Residency::Guest));
    g.put_u8(REQUEST_ENTRIES + 12 + 3, 2)
        .put_u8(REQUEST_ENTRIES + 12 + 2, 1);
    assert_eq!(residency(&mut g, RESOURCE), Ok(Residency::Guest));
    g.put_u8(REQUEST_ENTRIES + 12 + 2, 0).put_u32(LOAD_PHASE, 1);
    assert_eq!(
        residency(&mut g, RESOURCE),
        Ok(Residency::Guest),
        "phase 1, filter 0: a blocking wait"
    );
    g.put_u32(LOAD_FILTER, 2);
    assert_eq!(residency(&mut g, RESOURCE), Ok(Residency::Absent));
    g.put_u32(RESOURCE + 8, 0);
    assert_eq!(
        residency(&mut g, RESOURCE),
        Ok(Residency::Absent),
        "an empty resource"
    );
}

#[test]
fn the_aperture_step_needs_dialogue_on() {
    let mut g = guest();
    g.put_u8(DIALOGUE_AT + DIALOGUE_ON, 0);
    run(&mut g, &call(0.1)).unwrap();
    assert_eq!(aperture_now(&g), 0.0);
}

#[test]
fn a_facial_sequence_blends_into_the_unit_s_dialogue_state() {
    let mut g = guest();
    // Descriptor 2's facial sequence: 0x40 bytes at data + 0x100, one
    // track of one key at 0.5.
    let seq = DATA + 0x100;
    g.put_u32(DESCRIPTORS + 2 * 16 + 8, 0x100)
        .put_u32(DESCRIPTORS + 2 * 16 + 0xc, 0x40);
    g.put_u16(seq, 1)
        .put_u16(seq + 2, facial::VERSION)
        .put_f32(seq + 0xc, 1.0);
    g.put_u16(seq + 0x10, 1)
        .put_u16(seq + 0x12, 0)
        .put_u16(seq + 0x14, 0)
        .put_u16(seq + 0x16, 5000);
    g.put_f32(facial::PREVIOUS_SCALE, 1.0 / 128.0);
    g.put_u32(facial::GATE, DIALOGUE_AT)
        .put_u16(OBJECT + STATE + 0x36, 0);
    g.put_f32(UNIT_DEF + 0x1e4, 1.0);
    run(&mut g, &call(0.1)).unwrap();
    assert_eq!(g.u32(OBJECT + STATE + 0x38), Ok(TAG));
    assert_eq!(g.f32(OBJECT + STATE + 0x30 + 0xc), Ok(0.1));
    assert_eq!(g.u8(OBJECT + STATE + 0x30 + 0x20), Ok(64));
}
