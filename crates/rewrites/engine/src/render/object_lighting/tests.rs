use super::*;
use crate::FlatMemory;
use crate::datum::layout;

const FPU: Fpu = Fpu {
    mxcsr: 0x1f80,
    x87_control: 0x027f,
};
const OBJECT: u32 = 0x0001_0002;
const HEADERS: u32 = 0x2000;
const BODY: u32 = 0x3000;
const ARRAY: u32 = 0x10000;
const BITMAP: u32 = 0x10100;
const DATA: u32 = 0x11000;
const TAG_ARRAY: u32 = 0x20000;
const DEFINITION: u32 = 0x21000;

fn put_f64(m: &mut FlatMemory, at: u32, v: f64) {
    m.write(at, &v.to_le_bytes()).unwrap();
}

/// One root object, an empty two-record array and the retail constants
/// the ports read.
fn world() -> FlatMemory {
    let mut m = FlatMemory::new(0, 0x60_0000);
    m.set_u32(OBJECTS, 0x1000).unwrap();
    m.set_u32(0x1000 + 0x44, HEADERS).unwrap();
    m.set_u32(HEADERS + 2 * 12 + 8, BODY).unwrap();
    m.set_u32(BODY, 0x0000_0003).unwrap();
    m.set_u32(BODY + 0x14, NONE).unwrap();
    m.set_u32(BODY + 0xcc, NONE).unwrap();
    // Three permutation bytes and two change colours.
    m.set_u16(BODY + 0x118, 30).unwrap();
    m.set_u16(BODY + 0x11a, 0x200).unwrap();
    m.write(BODY + 0x200, &[1, 2, 3]).unwrap();
    m.set_u16(BODY + 0x124, 48).unwrap();
    m.set_u16(BODY + 0x126, 0x300).unwrap();
    for (i, c) in [[1.0, 0.5, 0.0], [0.25, 0.0, 2.0]].into_iter().enumerate() {
        for (j, v) in c.into_iter().enumerate() {
            m.set_f32(BODY + 0x300 + (2 + i as u32) * 12 + 4 * j as u32, v)
                .unwrap();
        }
    }
    m.set_u32(TAGS, TAG_ARRAY).unwrap();
    m.set_u32(TAG_ARRAY + 3 * 16 + 8, DEFINITION).unwrap();
    m.set_u32(CACHE, ARRAY).unwrap();
    m.set_u32(ARRAY + layout::MAXIMUM_COUNT, 2).unwrap();
    m.set_u32(ARRAY + layout::ELEMENT_SIZE, 0x100).unwrap();
    m.set_u16(ARRAY + layout::NEXT_SALT, 0x8000).unwrap();
    m.set_u32(ARRAY + layout::DATA, DATA).unwrap();
    m.set_u32(ARRAY + layout::BITMAP, BITMAP).unwrap();
    m.set_u32(FRAME, 100).unwrap();
    m.set_f32(OLDEST_FLOOR, 0.0).unwrap();
    m.set_f32(FUTURE_AGE, 1000.0).unwrap();
    put_f64(&mut m, LIGHT_ELEVATION, 0.0);
    put_f64(&mut m, LIGHT_AZIMUTH, 0.0);
    m.set_f32(AMBIENT, 0.3).unwrap();
    m.set_f32(DIFFUSE, 0.7).unwrap();
    m.set_f32(STRENGTH, 1.5).unwrap();
    m.set_f32(BYTE_SCALE, 255.0).unwrap();
    m.set_u32(DEFAULT_LIGHTING, 0x30000).unwrap();
    m
}

/// Both records live and owned by other objects, stamped `stamps`.
fn full(m: &mut FlatMemory, stamps: [u32; 2]) {
    m.set_u32(ARRAY + layout::HIGH_WATER, 2).unwrap();
    m.set_u32(ARRAY + layout::ACTIVE_COUNT, 2).unwrap();
    m.set_u32(BITMAP, 0b11).unwrap();
    for (i, stamp) in stamps.into_iter().enumerate() {
        let entry = DATA + 0x100 * i as u32;
        m.set_u16(entry, 0x8001 + i as u16).unwrap();
        m.set_u32(entry + 4, 0x0005_0009).unwrap();
        m.set_u32(entry + 0xc, stamp).unwrap();
    }
}

#[test]
fn a_new_record_is_the_objects_and_gives_no_lighting_until_lit() {
    let mut m = world();
    assert_eq!(
        sample(&mut m, OBJECT, FPU),
        Ok(Sample { eax: 0, edx: DATA })
    );
    let handle = 0x8000_0000;
    let entry = DATA;
    assert_eq!(m.u32(BODY + 0xcc).unwrap(), handle);
    assert_eq!(m.u32(ARRAY + layout::ACTIVE_COUNT).unwrap(), 1);
    assert_eq!(m.u32(entry + 4).unwrap(), OBJECT);
    assert_eq!(m.u32(entry + 8).unwrap(), NONE);
    assert_eq!(m.u32(entry + 0xc).unwrap(), NONE);
    assert_eq!(m.u16(entry + 2).unwrap(), 0);
    assert_eq!(m.u32(entry + 0x44).unwrap(), OBJECT);
    assert_eq!(m.u32(entry + 0x48).unwrap(), CALLBACK);
    assert_eq!(m.u32(entry + 0xbc).unwrap(), NONE);
    // The default light points along +x: elevation and azimuth 0.
    let light = entry + 0x44;
    assert_eq!(m.f32(light + 0x10).unwrap(), 0.3);
    assert_eq!(m.f32(light + 0x50).unwrap(), 0.3);
    assert_eq!(m.f32(light + 0x30).unwrap(), 0.7);
    assert_eq!(m.f32(light + 0x2c).unwrap(), 1.5);
    assert_eq!(m.f32(light + 0x3c).unwrap(), 1.0);
    assert_eq!(m.f32(light + 0x1c).unwrap(), 1.0);
    assert_eq!(m.u32(light + 0x40).unwrap(), 0);
    assert_eq!(m.u32(light + 0x44).unwrap(), 0);
    assert_eq!(m.f32(light + 0x54).unwrap(), -1.0);
    assert_eq!(m.u32(light + 0x58).unwrap(), 0, "0 - 0 is +0");
    assert_eq!(m.u16(light + 0x60).unwrap(), 0xffff);
    for view in 0..4 {
        assert_eq!(
            m.bytes(entry + 0xc0 + view * 0x10, 4).unwrap(),
            [1, 2, 3, 0]
        );
    }
    // 127.5 rounds to even; 2.0 * 255 wraps to 0xfe.
    assert_eq!(m.u8(entry + 0xb8).unwrap(), 2);
    assert_eq!(m.u32(entry + 0xa8).unwrap(), 0x00ff_8000);
    assert_eq!(m.u32(entry + 0xac).unwrap(), 0x0040_00fe);
    assert_eq!(m.u32(entry + 0xb0).unwrap(), 0);
}

#[test]
fn the_default_light_direction_is_the_x87s() {
    let mut m = world();
    put_f64(&mut m, LIGHT_ELEVATION, 0.5);
    put_f64(&mut m, LIGHT_AZIMUTH, 0.25);
    record(&mut m, OBJECT, FPU).unwrap();
    let light = DATA + 0x44;
    let want = [
        0.25f64.cos() * 0.5f64.cos(),
        0.25f64.sin() * 0.5f64.cos(),
        0.5f64.sin(),
    ];
    for (i, w) in want.into_iter().enumerate() {
        let d = m.f32(light + 0x3c + 4 * i as u32).unwrap();
        assert!(
            (d - w as f32).abs() <= f32::EPSILON,
            "axis {i}: {d} against {w}"
        );
        assert_eq!(m.f32(light + 0x54 + 4 * i as u32).unwrap(), -d);
        assert_eq!(m.f32(light + 0x1c + 4 * i as u32).unwrap(), d);
    }
}

#[test]
fn the_objects_own_record_is_given_as_it_is() {
    let mut m = world();
    let handle = record(&mut m, OBJECT, FPU).unwrap();
    let before = m.bytes.clone();
    assert_eq!(record(&mut m, OBJECT, FPU), Ok(handle));
    assert!(m.bytes == before, "a hit writes nothing");
}

#[test]
fn a_lit_record_gives_its_lighting_and_the_records_base_in_edx() {
    let mut m = world();
    record(&mut m, OBJECT, FPU).unwrap();
    m.set_u8(DATA + 3, 1).unwrap();
    assert_eq!(
        sample(&mut m, OBJECT, FPU),
        Ok(Sample {
            eax: DATA + 0x54,
            edx: DATA
        })
    );
}

#[test]
fn a_default_lit_object_takes_the_default_lighting_and_makes_no_record() {
    let mut m = world();
    m.set_u8(BODY + 7, 1).unwrap();
    let before = m.bytes.clone();
    assert_eq!(
        sample(&mut m, OBJECT, FPU),
        Ok(Sample {
            eax: 0x30000 + 0x1c,
            edx: HEADERS | 1
        })
    );
    assert!(m.bytes == before);
}

#[test]
fn a_full_array_takes_over_the_oldest_record() {
    let mut m = world();
    full(&mut m, [90, 50]);
    assert_eq!(record(&mut m, OBJECT, FPU), Ok(0x8002_0001));
    let entry = DATA + 0x100;
    assert_eq!(m.u32(entry + 4).unwrap(), OBJECT);
    assert_eq!(m.u16(entry).unwrap(), 0x8002, "the salt stays");
    assert_eq!(m.u32(BODY + 0xcc).unwrap(), 0x8002_0001);
    assert_eq!(
        m.u32(DATA + 4).unwrap(),
        0x0005_0009,
        "the younger record is left"
    );
}

#[test]
fn a_record_stamped_ahead_of_the_frame_counts_as_the_future_age() {
    let mut m = world();
    full(&mut m, [150, 50]);
    assert_eq!(record(&mut m, OBJECT, FPU), Ok(0x8001_0000));
}

#[test]
fn a_full_array_with_nothing_past_the_floor_gives_none_and_writes_nothing() {
    let mut m = world();
    full(&mut m, [90, 50]);
    m.set_f32(OLDEST_FLOOR, 100.0).unwrap();
    let before = m.bytes.clone();
    assert_eq!(record(&mut m, OBJECT, FPU), Ok(NONE));
    assert_eq!(sample(&mut m, OBJECT, FPU), Ok(Sample { eax: 0, edx: 0 }));
    assert!(m.bytes == before);
}

#[test]
fn unforced_colours_follow_the_definitions_bit_and_the_bodys_count() {
    let mut m = world();
    record(&mut m, OBJECT, FPU).unwrap();
    let entry = DATA;
    m.set_u8(entry + 0xb8, 9).unwrap();
    colours(&mut m, OBJECT, false, FPU).unwrap();
    assert_eq!(m.u8(entry + 0xb8).unwrap(), 9, "no colours asked for");
    m.set_u8(DEFINITION + 0x1c, 1).unwrap();
    // 47 bytes: one colour pair, its second half at +12.
    m.set_u16(BODY + 0x124, 47).unwrap();
    for (j, v) in [0.25, 0.0, 2.0].into_iter().enumerate() {
        m.set_f32(BODY + 0x30c + 4 * j as u32, v).unwrap();
    }
    colours(&mut m, OBJECT, false, FPU).unwrap();
    assert_eq!(m.u8(entry + 0xb8).unwrap(), 1);
    assert_eq!(m.u32(entry + 0xa8).unwrap(), 0x0040_00fe);
}

#[test]
#[should_panic(expected = "residency: lighting sample of a child object")]
fn a_child_objects_sample_is_a_named_gap() {
    let mut m = world();
    m.set_u32(BODY + 0x14, 0x0001_0001).unwrap();
    let _ = sample(&mut m, OBJECT, FPU);
}
