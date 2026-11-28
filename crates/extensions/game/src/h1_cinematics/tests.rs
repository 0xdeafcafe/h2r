use super::*;
use guest_layout::testing::FakeGuest;

fn globals(m: &mut FakeGuest) -> u32 {
    let (sc, defs, pool, data, clock) = (0x10000, 0x20000, 0x30000, 0x40000, 0x50000);
    let n = Field::ALL.len() as u32;
    let cells = data + 8 * hs_pool::EXTERNALS + 4;
    m.map(points::SCENARIO.base, 4)
        .put_u32(points::SCENARIO.base, sc)
        .map(sc, 0x200)
        .put_u32(sc + 0x1c0, n + 5)
        .put_u32(sc + 0x1c4, defs)
        .map(defs, (n + 5) * 40)
        .map(hs_pool::GLOBALS, 4)
        .put_u32(hs_pool::GLOBALS, pool)
        .map(pool, layout::HEADER_SIZE)
        .put_u32(pool + layout::HIGH_WATER, hs_pool::EXTERNALS + n + 5)
        .put_u32(pool + layout::DATA, data)
        .map(cells, (n + 5) * 8)
        .map(points::GAME_TIME.base, 4)
        .put_u32(points::GAME_TIME.base, clock)
        .map(clock, 16);
    m.write(defs, Field::Active.name().as_bytes()).unwrap();
    m.write(defs + n * 40, MARKER.as_bytes()).unwrap();
    for (i, marker) in [
        hs::h1_ai_slots::HOOK_STATE,
        hs::h1_ai_slots::timers::PHASED_GLOBAL,
        hs::h1_ai_slots::TELEPORT_STATE,
        hs::h1_ai_slots::PERCEPTION_STATE,
    ].iter().enumerate() {
        m.write(defs + (n + 1 + i as u32) * 40, marker.as_bytes()).unwrap();
    }
    cells
}

#[test]
fn lifepod_blur_follows_the_checkpointed_campaign_clock() {
    let mut m = FakeGuest::new();
    let cells = globals(&mut m);
    for (f, word) in [
        (Field::Active, 1),
        (Field::Passes, 1),
        (Field::Kernel, 1),
        (Field::RadiusFrom, 3f32.to_bits()),
        (Field::RadiusTo, 0),
        (Field::BlurSeconds, 7f32.to_bits()),
        (Field::BlurTick, 411),
    ] {
        m.put_u32(cells + 8 * f as u32, word);
    }
    for (tick, radius) in [
        (411, Some(3.)),
        (516, Some(1.5)),
        (621, None),
        (412, Some(3. * (1. - 1. / 210.))),
    ] {
        m.put_u32(0x50008, tick);
        assert_eq!(Frame::read(&m).map(|f| f.radius), radius);
    }
    m.put_u32(cells, 0xffff_ff00);
    assert_eq!(Frame::read(&m), None);
}

#[test]
fn cinematic_filter_and_video_read_their_own_parameters() {
    let mut m = FakeGuest::new();
    let cells = globals(&mut m);
    for (f, word) in [
        (Field::Active, 1),
        (Field::DesatTo, 1f32.to_bits()),
        (Field::LightFrom, 1f32.to_bits()),
        (Field::FilterSeconds, 2f32.to_bits()),
        (Field::FilterFlag, 1),
        (Field::Red, 0.5f32.to_bits()),
        (Field::Blue, 1f32.to_bits()),
    ] {
        m.put_u32(cells + 8 * f as u32, word);
    }
    m.put_u32(0x50008, 30);
    let f = Frame::read(&m).unwrap();
    assert_eq!(
        (f.light, f.desat, f.tint, f.filtered),
        (0.5, 0.5, [0.5, 0., 1.], true)
    );
    m.put_u32(cells + 8 * Field::Red as u32, 0)
        .put_u32(cells + 8 * Field::Blue as u32, 0);
    assert_eq!(Frame::read(&m).unwrap().tint, [0., 1., 0.]);
    m.put_u32(cells + 8 * Field::Video as u32, 1)
        .put_u32(cells + 8 * Field::VideoMode as u32, 2)
        .put_u32(cells + 8 * Field::Noise as u32, 0.75f32.to_bits());
    let f = Frame::read(&m).unwrap();
    assert_eq!((f.video, f.gain, f.noise), (true, 4., 0.75));
}

#[test]
fn light_reset_preserves_live_records_their_owners_and_registered_lights() {
    let mut m = FakeGuest::new();
    let (pool, bits, rows) = (0x10000, 0x20000, 0x30000);
    m.map(OBJECT_RENDER_STATES, 4)
        .put_u32(OBJECT_RENDER_STATES, pool)
        .map(pool, layout::HEADER_SIZE)
        .put_u32(pool + layout::ELEMENT_SIZE, 0x100)
        .put_u32(pool + layout::DATA, rows)
        .put_u32(pool + layout::BITMAP, bits)
        .put_u32(pool + layout::HIGH_WATER, 3)
        .map(bits, 4)
        .put_u32(bits, 5)
        .map(rows, 0x300);
    m.write(rows, &[0x55; 0x300]).unwrap();
    let mut expected = m.bytes(rows, 0x300).unwrap();
    for i in [0, 0x200] {
        expected[i + 2] = 0;
        expected[i + 8..i + 12].fill(0xff);
    }
    reset_light_caches(&mut m).unwrap();
    assert_eq!(m.bytes(rows, 0x300).unwrap(), expected);
    assert_eq!(m.u32(bits), Ok(5));
}

#[test]
fn model_reflection_changes_both_endpoints_and_restores_the_authored_material() {
    use h1_contract::shader::MODEL_REFLECTION_SIGNATURE;
    let mut m = FakeGuest::new();
    let cells = globals(&mut m);
    let (table, shad, pp, colors, vectors) = (0x60000, 0x70000, 0x80000, 0x90000, 0xa0000);
    let tag = 0x8000_0000;
    m.map(engine::screen_effect::TAG_TABLE, 4)
        .put_u32(engine::screen_effect::TAG_TABLE, table)
        .map(table, 16)
        .put_u32(table, u32::from_be_bytes(*b"shad"))
        .put_u32(table + 4, tag)
        .put_u32(table + 8, shad)
        .map(shad, 0x28)
        .put_u32(shad + 0x20, 1)
        .put_u32(shad + 0x24, pp)
        .map(pp, 0x20)
        .put_u32(pp + 0xc, 13)
        .put_u32(pp + 0x10, colors)
        .put_u32(pp + 0x18, vectors)
        .map(colors, 52)
        .map(vectors, 192)
        .put_u32(colors + 48, MODEL_REFLECTION_SIGNATURE);
    let authored = [0x80100821, 0x00407235, 0x40000000, 0x90000000];
    for (i, value) in authored.into_iter().enumerate() {
        m.put_u32(colors + 32 + i as u32 * 4, value);
    }
    for (f, v) in [
        (Field::Reflect0, 1f32),
        (Field::Reflect1, 0.5),
        (Field::Reflect2, 0.),
        (Field::Reflect3, 0.25),
    ] {
        m.put_u32(cells + 8 * f as u32, v.to_bits());
    }
    material(&mut m, tag).unwrap();
    assert_eq!(m.u32(colors + 16), Ok(0xff8000));
    assert_eq!(m.u32(colors + 20), Ok(0xff8000));
    assert_eq!(m.f32(vectors + 6 * 16 + 4), Ok(0.5));
    assert_eq!(m.f32(vectors + 9 * 16 + 12), Ok(0.25));
    for f in [
        Field::Reflect0,
        Field::Reflect1,
        Field::Reflect2,
        Field::Reflect3,
    ] {
        m.put_u32(cells + 8 * f as u32, 0);
    }
    material(&mut m, tag).unwrap();
    for (i, v) in authored.into_iter().enumerate() {
        assert_eq!(m.u32(colors + 16 + i as u32 * 4), Ok(v));
    }
}

#[test]
fn video_noise_uses_the_two_retail_draws_for_each_frame() {
    use render_bridge::game_random::{RANDOM_A, RANDOM_C};
    let mut seed = 0u32;
    for tick in 0..200 {
        let pair = std::array::from_fn(|_| {
            seed = seed.wrapping_mul(RANDOM_A).wrapping_add(RANDOM_C);
            (seed >> 16) as f32 / 65535.
        });
        assert_eq!(video_offset(tick), pair);
    }
}
