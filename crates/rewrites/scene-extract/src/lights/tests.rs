use super::*;
use guest_layout::FlatMemory;
use guest_layout::datum::layout as datum;
use engine::render::light::layout as record;
use frame_scene::{FrameScene, RenderCamera, SceneBuilder, SceneId, Stamp, ViewKey, ViewKind};
use std::sync::Arc;

const ARRAY: u32 = 0x0058_0000;
const RECORDS: u32 = 0x0058_1000;
const TAGS: u32 = 0x0059_0000;
const CONE: u32 = 0x0059_1000;
const SPHERE: u32 = 0x0059_2000;
const VIEW: u32 = 0x0059_3000;
const TAIL: u32 = 0x0059_4000;
const LIST: u32 = 0x0059_5000;
const HANDLES: u32 = 0x0059_6000;
const CONE_TAG: u32 = 0x7777_0000;
const SPHERE_TAG: u32 = 0x7777_0001;
const DRAWN: [u32; 5] = [handle(7), handle(4), handle(12), handle(2), handle(19)];

const FPU: lc::Fpu = lc::Fpu { mxcsr: 0x1f80, x87_control: 0x027f };

const fn handle(index: u32) -> u32 {
    0xb123_0000 | index
}

fn floats(m: &mut FlatMemory, address: u32, values: &[f32]) {
    for (i, &value) in values.iter().enumerate() {
        m.set_f32(address + 4 * i as u32, value).unwrap();
    }
}

fn guest() -> FlatMemory {
    let mut m = FlatMemory::new(0x0044_0000, 0x0016_0000);
    for (addr, value) in [
        (lc::constant::ONE, 1.0),
        (lc::constant::MINUS_ONE, -1.0),
        (lc::constant::HALF, 0.5),
        (lc::constant::TWO, 2.0),
        (lc::constant::QUARTER, 0.25),
        (0x0045_dbdc, 0.0001), // volume builder's epsilon
        (MIN_DEPTH, 0.25),
        (PIXEL_SCALE, 16.0),
        (VIEW_DEPTH[0], 1.0),
        (sc::constant::SIZE_A, 0.41),
        (sc::constant::SIZE_B, 0.27),
        (sc::constant::SIZE_MIN, 96.0),
        (sc::constant::DEPTH_RANGE, 135.0),
        (sc::constant::NEAR_STEP, 0.18),
        (sc::constant::PLANE_SCALE, 0.37),
        (sc::constant::PLANE_OFFSET, 0.51),
    ] {
        m.set_f32(addr, value).unwrap();
    }
    for (addr, value) in [
        (lc::global::FRAME_TAIL, TAIL),
        (lc::global::VIEW, VIEW),
        (light::ARRAY_POINTER, ARRAY),
        (ARRAY + datum::MAXIMUM_COUNT, light::MAXIMUM_COUNT as u32),
        (ARRAY + datum::ELEMENT_SIZE, light::RECORD_SIZE as u32),
        (ARRAY + datum::HIGH_WATER, 20),
        (ARRAY + datum::DATA, RECORDS),
        (guest_layout::tags::TAG_ENTRIES_GLOBAL, TAGS),
        (MAIN_LIGHT_LIST, LIST),
        (LIST, 32),
        (LIST + HANDLE_COLUMN, HANDLES),
        (lp::global::SCENE_FLAGS, 1 << 10),
    ] {
        m.set_u32(addr, value).unwrap();
    }
    m.set_u16(LIST + COUNT, 22).unwrap();
    for (datum, definition, kind) in [(CONE_TAG, CONE, 2), (SPHERE_TAG, SPHERE, 0)] {
        let addr = TAGS + 16 * u32::from(datum as u16);
        m.set_u32(addr, u32::from_be_bytes(*b"ligh")).unwrap();
        m.set_u32(addr + 4, datum).unwrap();
        m.set_u32(addr + 8, definition).unwrap();
        m.set_u16(definition + 4, kind).unwrap();
        m.set_u32(definition + lp::DEFINITION_GEL, u32::MAX).unwrap();
        m.set_u32(definition + lp::DEFINITION_GEL_SHADER, u32::MAX).unwrap();
    }
    m.set_u32(CONE, light::definition_flags::NO_SHADOW).unwrap();
    floats(&mut m, CONE + 0x20, &[0.5, 0.75, 0.7, 1.0, 10.0]);
    floats(&mut m, SPHERE + 0x18, &[2.0, 3.0]);
    floats(&mut m, TAIL, &[1.0, 2.0, 3.0]);

    for i in 0..20 {
        let at = RECORDS + i * light::RECORD_SIZE as u32;
        let def = if i % 2 == 0 { CONE_TAG } else { SPHERE_TAG };
        let p = i as f32 + 2.0;
        m.set_u16(at + record::SALT as u32, (handle(i) >> 16) as u16).unwrap();
        m.set_u16(at + record::FLAGS as u32, light::flags::LIVE).unwrap();
        m.set_u32(at + record::DEFINITION as u32, def).unwrap();
        m.set_u32(at + 8, u32::MAX).unwrap(); // No placement override.
        m.set_u32(at + record::PARENT_OBJECT as u32, u32::MAX).unwrap();
        floats(&mut m, at + record::BOUNDS_CENTRE as u32, &[p, 0.0, 0.0, 3.0]);
        floats(&mut m, at + record::POSITION as u32, &[p, 0.0, 0.0]);
        floats(&mut m, at + record::TARGET as u32, &[p + 1.0, 0.0, 0.0]);
        floats(&mut m, at + record::FORWARD as u32, &[1.0, 0.0, 0.0]);
        floats(&mut m, at + record::UP as u32, &[0.0, 0.0, 1.0]);
        floats(&mut m, at + 0xc4, &[1.0]);
        floats(&mut m, at + record::INTENSITY as u32, &[0.8, 0.5, 0.3, 0.1, 0.1, 0.2, 0.3, 1.0]);
        floats(&mut m, at + 0xf8, &[0.25, 0.8, 0.6, 0.7, 0.45]);
        m.set_u32(HANDLES + 4 * (i + 1), handle(i)).unwrap();
    }
    m.set_u32(HANDLES, handle(1) ^ (1 << 16)).unwrap();
    m.set_u32(HANDLES + 4 * 21, handle(0)).unwrap();
    m
}

fn extract(m: &FlatMemory) -> Arc<FrameScene> {
    let mut builder = SceneBuilder::default();
    builder.begin(SceneId { tick: 41, ..Default::default() });
    builder.open_view(
        ViewKey { kind: ViewKind::Game, player: 0, view: 0 },
        RenderCamera {
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov: 1.0,
            projection_xy: None,
            viewport: [0, 0, 480, 640],
            near: 0.1,
            far: 100.0,
        },
    ).unwrap();
    for handle in DRAWN {
        let definition = if handle % 2 == 0 { CONE_TAG } else { SPHERE_TAG };
        builder.push_light(Light { handle, definition, guest_weight: Some(0.25), ..Default::default() }, &[]).unwrap();
    }
    let candidates = candidates(m, FPU, handles(m).as_deref(), &DRAWN, |_, _, _| Some(true)).unwrap();
    builder.add_light_candidates(candidates).unwrap();
    builder.seal(Stamp::default()).unwrap()
}

#[test]
fn twenty_candidates_keep_the_guests_five_and_both_constant_sets() {
    let m = guest();
    let before = m.bytes.clone();
    let s = extract(&m);
    let subset = candidates(&m, FPU, handles(&m).as_deref(), &DRAWN, |address, _, _| {
        Some(address != RECORDS && address != RECORDS + 7 * light::RECORD_SIZE as u32)
    }).unwrap();
    assert_eq!(subset.len(), 19);
    assert!(!subset.iter().any(|light| light.handle == handle(0)));
    let drawn = subset.iter().find(|light| light.handle == handle(7)).unwrap();
    assert_eq!(drawn.candidate.as_ref().unwrap().view_eligible, Some(false));
    s.check().unwrap();
    assert_eq!(m.bytes, before);
    let v = &s.views[0];
    let counts = s.counts(v);
    assert_eq!((counts.lights_candidates, counts.lights, counts.lights_with_constants), (20, 5, 20));
    let lights = s.lights_of(v);
    assert_eq!(lights.iter().filter(|l| l.guest_selected).map(|l| l.handle).collect::<Vec<_>>(), DRAWN);
    for light in lights {
        let c = light.candidate.as_ref().unwrap();
        assert_eq!(c.intensity, 0.8);
        assert_eq!(c.weight, 0.6);
        assert_eq!(c.volume.as_ref().unwrap().kind(), c.definition_kind);
        assert_eq!(c.view_eligible, Some(true));
        assert_eq!(c.bounds.0, c.position);
        assert!(c.priority.unwrap() > 0.0);
        assert_eq!(light.guest_selected, light.guest_weight.is_some());
        let sets = c.constants.as_ref().unwrap();
        assert!(!sets.unshadowed.shadowed());
        assert_eq!(sets.unshadowed.frame, sets.shadowed.frame);
        if light.definition == CONE_TAG {
            assert_ne!(sets.shadowed.definition_flags & light::definition_flags::NO_SHADOW, 0);
            assert!(sets.shadowed.shadowed());
            let projection = sets.projection.as_ref().unwrap();
            assert!(projection.size.into_iter().all(|extent| extent > 0));
            assert_ne!(projection.rows[0][0], 0);
        } else {
            assert_eq!(sets.unshadowed, sets.shadowed);
            assert_eq!(sets.projection, None);
        }
    }
}

#[test]
fn light_candidate_scene_hash_is_independent_of_lighting_setting() {
    let hash = |mode| {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "lights::tests::light_candidate_scene_hash_probe", "--nocapture"])
            .env("H2_LIGHTING", mode)
            .output()
            .unwrap();
        assert!(output.status.success(), "{mode}: {}", String::from_utf8_lossy(&output.stdout));
        String::from_utf8(output.stdout).unwrap().lines()
            .find_map(|line| line.strip_prefix("scene hash "))
            .unwrap().to_owned()
    };
    assert_eq!(hash("retail"), hash("e3"));
}

#[test]
#[ignore = "subprocess of light_candidate_scene_hash_is_independent_of_lighting_setting"]
fn light_candidate_scene_hash_probe() {
    println!("scene hash {:016x}", frame_scene::summary_hash(&extract(&guest())));
}
