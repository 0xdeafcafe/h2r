use std::sync::Mutex;

use guest_layout::memory::{FlatMemory, GuestMemory};

use super::*;

/// The counters and the pass hooks' state are global: these tests take turns.
static TURN: Mutex<()> = Mutex::new(());

const BASE: u32 = 0x004a_0000;
const ESP: u32 = 0x0090_0000;
const HESP: u32 = 0x0090_0100;
const SLOT: u32 = 0x0090_1000;
const DATA: u32 = 0x0090_2000;
const TABLE: u32 = 0x0060_0000;
const IMPS: u32 = 0x0063_2000;
const RECORD_INDEX: u32 = 5;
const ARGUMENT: u32 = (RECORD_INDEX << 8) | 2;
const SHADER: u32 = 1;

fn counter(name: &str) -> u64 {
    super::super::tested(name)
}

fn kept(name: &str) -> u64 {
    crate::native_layers::counters()
        .into_iter()
        .find(|(k, _)| *k == name)
        .map_or(0, |(_, v)| v)
}

/// A BSP part drawn by a shader whose template has two layer-15 passes.
fn world() -> FlatMemory {
    let mut m = FlatMemory::new(BASE, 0x0070_0000);
    let w = |m: &mut FlatMemory, at: u32, v: u32| m.set_u32(at, v).unwrap();
    w(&mut m, guest_layout::tags::TAG_ENTRIES_GLOBAL, TABLE);
    for (i, group, data) in [
        (1, *b"shad", 0x0061_0000),
        (2, *b"stem", 0x0062_0000),
        (3, *b"spas", 0x0063_0000),
        (4, *b"vrtx", 0x0064_0000),
    ] {
        w(&mut m, TABLE + i * 16, u32::from_be_bytes(group));
        w(&mut m, TABLE + i * 16 + 8, data);
    }
    w(&mut m, 0x0061_0000 + 0x24, 0x0061_1000);
    w(&mut m, 0x0061_1000, 2);
    w(&mut m, 0x0062_0000 + 0x5c, 0x0062_1000);
    for (at, v) in [(4, 0x0062_2000), (12, 0x0062_3000), (20, 0x0062_4000)] {
        w(&mut m, 0x0062_1000 + at, v);
    }
    // Layer 15: two passes from pass 1.
    m.set_u16(0x0062_3000 + 15 * 2, (2 << 9) | 1).unwrap();
    for pass in 1..=2 {
        w(&mut m, 0x0062_4000 + pass * 10 + 4, 3);
    }
    w(&mut m, 0x0063_0000 + 0x20, 0x0063_1000);
    w(&mut m, 0x0063_1000 + 4, IMPS);
    w(&mut m, IMPS + 0x100, 4);
    w(&mut m, 0x0064_0000 + 4, 3);
    w(&mut m, 0x0064_0000 + 8, 0x0064_1000);
    w(&mut m, 0x0064_1000 + 12, 0x10);
    // The BSP's section 0, part count 4, vertex class 0.
    w(&mut m, crate::native_world::ACTIVE_BSP, 0x0065_0000);
    w(&mut m, 0x0065_0000 + 0xa0, 0x0065_1000);
    w(&mut m, 0x0065_1000 + 0x50, 0x0065_2000);
    w(&mut m, 0x0065_2000, 4);
    let record = engine::render_loop::opaque_material::RENDER_RECORDS + RECORD_INDEX * 0x20;
    w(&mut m, record + 4, u32::MAX);
    w(&mut m, record + 0x10, 0xabcd_1234);
    // The build's call and the handler's.
    for (i, v) in [BUILD_RETURN, SHADER, 0, 0, LAYER, 0, ARGUMENT, SLOT].into_iter().enumerate() {
        w(&mut m, ESP + 4 * i as u32, v);
    }
    w(&mut m, HESP + 4, DATA);
    w(&mut m, DATA, SHADER);
    w(&mut m, DATA + 4, ARGUMENT);
    m
}

fn frame(m: &mut FlatMemory, frame: u32) {
    m.set_u32(view_update::FRAME, frame).unwrap();
}

fn select(m: &mut FlatMemory, pass: u32) {
    m.set_u32(ESP + 4 * 5, pass).unwrap();
}

#[test]
fn the_build_makes_the_slot_from_the_records_chain() {
    let _t = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let m = world();
    assert_eq!(passes(&m, SHADER), Ok(2));
    let built = predict(&m, &args(&m, ESP).unwrap()).unwrap();
    assert!(built.al);
    assert_eq!(built.slot[4], 0, "the implementation");
    assert_eq!(built.slot[5], 1);
    assert_eq!(built.slot[8..10], [0, 0], "the vertex class");
    assert_eq!(built.slot[16..20], IMPS.to_le_bytes());
    assert_eq!(built.slot[20..24], 0xabcd_1234u32.to_le_bytes());
}

#[test]
fn only_the_handlers_layer_15_call_is_the_arms() {
    let _t = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let mut m = world();
    assert!(args(&m, ESP).is_some());
    m.set_u32(ESP, BUILD_RETURN + 4).unwrap();
    assert_eq!(args(&m, ESP), None);
    m.set_u32(ESP, BUILD_RETURN).unwrap();
    m.set_u32(ESP + 16, 3).unwrap();
    assert_eq!(args(&m, ESP), None);
}

#[test]
fn what_the_model_cannot_make_is_a_named_refusal() {
    let _t = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let mut m = world();
    frame(&mut m, 5);
    m.set_u32(TABLE + 16, u32::from_be_bytes(*b"PRTM")).unwrap();
    let before = kept("scene_kept_15_geometry_particle_material");
    assert_eq!(build(&mut m, ESP), None, "the guest builds it");
    assert_eq!(kept("scene_kept_15_geometry_particle_material"), before + 1);
    assert_eq!(passes(&m, SHADER), Err(Refusal::ParticleMaterial));
    m.set_u32(TABLE + 16, u32::from_be_bytes(*b"foo ")).unwrap();
    assert_eq!(passes(&m, SHADER), Err(Refusal::NotShader));

    let mut m = world();
    m.set_u32(ESP + 4 * 6, 0x400 << 8).unwrap();
    assert_eq!(predict(&m, &args(&m, ESP).unwrap()), Err(Refusal::Record));
    let mut m = world();
    m.set_u32(0x0065_2000, 2).unwrap();
    assert_eq!(predict(&m, &args(&m, ESP).unwrap()), Err(Refusal::Layout), "a part past the geometry");
}

#[test]
fn every_frames_build_is_made_in_rust() {
    let _t = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let mut m = world();
    for f in [64, 65] {
        frame(&mut m, f);
        let answered = counter("answer_geometry");
        select(&mut m, 1);
        m.write(SLOT, &[0xee; 24]).unwrap();
        let expect = predict_from(&m, &args(&m, ESP).unwrap(), [0xee; 24]).unwrap();
        assert_eq!(build(&mut m, ESP), Some(true), "frame {f}");
        assert_eq!(counter("answer_geometry"), answered + 1);
        let mut written = [0; 24];
        m.read(SLOT, &mut written).unwrap();
        assert_eq!(written, expect.slot, "the slot is the model's, made in Rust");
    }
}

struct Host(Mutex<Vec<Request>>, Result<(), &'static str>);

impl AnswerHost for Host {
    fn draw(&self, _: &mut dyn GuestMemory, request: &Request) -> Result<(), &'static str> {
        self.0.lock().unwrap().push(*request);
        self.1
    }
}

#[test]
fn a_cluster_or_placement_entry_is_drawn_by_the_host_and_a_refusal_keeps_the_guests_handler() {
    use std::sync::Arc;
    let _t = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let mut m = world();
    let record = engine::render_loop::opaque_material::RENDER_RECORDS + RECORD_INDEX * 0x20;
    m.set_u32(record + 0xc, 0x7ff << 18).unwrap();
    m.set_u32(DATA - 4, 0x0004_1020).unwrap();
    let host = Arc::new(Host(Mutex::default(), Ok(())));
    set_answer_host(Some(host.clone()));

    frame(&mut m, 50);
    let drawn = counter("answer_geometry_draw");
    assert!(answer_handler(&mut m, HESP));
    assert_eq!(*host.0.lock().unwrap(), [Request { shader: SHADER, owner: Owner::Cluster(0), record: RECORD_INDEX as u16, part: 2, passes: 2 }]);
    assert_eq!(counter("answer_geometry_draw"), drawn + 1);

    // A host that refuses leaves the handler to the guest.
    set_answer_host(Some(Arc::new(Host(Mutex::default(), Err("warming")))));
    assert!(!answer_handler(&mut m, HESP));
    // A placement's entry is drawn as the placement's.
    set_answer_host(Some(host.clone()));
    m.set_u32(record + 0xc, 3 << 18).unwrap();
    assert!(answer_handler(&mut m, HESP));
    assert_eq!(host.0.lock().unwrap()[1].owner, Owner::Placement(3));
    // A pass the model does not accept keeps the guest's handler.
    m.set_u32(record + 0xc, 0x7ff << 18).unwrap();
    m.set_u32(0x0064_0000 + 4, 0).unwrap();
    assert!(!answer_handler(&mut m, HESP), "no vertex class to draw");
    assert_eq!(host.0.lock().unwrap().len(), 2);
    set_answer_host(None);
}

#[test]
fn an_object_entry_is_drawn_by_the_host_as_the_objects() {
    use std::sync::Arc;
    use scene_extract::layout::objects::{RECORD_LIGHTING, RECORD_MODEL};
    let _t = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let mut m = world();
    let record = engine::render_loop::opaque_material::RENDER_RECORDS + RECORD_INDEX * 0x20;
    m.set_u32(DATA - 4, 0x0004_1020).unwrap();
    // The record is an object's: a model, and the lighting its cache
    // record gives.
    let mut obj = scene_extract::testing::Mem::default();
    let handle = scene_extract::testing::object(&mut obj, 4, 0xe000_0100, 1);
    for (at, v) in obj.0 {
        m.set_u32(at, v).unwrap();
    }
    m.set_u32(record + RECORD_MODEL, 0xe100_0000 | RECORD_INDEX).unwrap();
    // The build reads that model's tag: a `mode` whose section 0 has the
    // BSP world's geometry (four parts) and vertex class 0.
    m.set_u32(TABLE + RECORD_INDEX * 16, u32::from_be_bytes(*b"mode")).unwrap();
    m.set_u32(TABLE + RECORD_INDEX * 16 + 8, 0x0066_0000).unwrap();
    m.set_u32(0x0066_0000 + 0x24, 1).unwrap();
    m.set_u32(0x0066_0000 + 0x28, 0x0066_1000).unwrap();
    m.set_u32(0x0066_1000 + 0x34, 0x0065_2000).unwrap();
    m.set_u32(record + RECORD_LIGHTING, scene_extract::testing::lighting(4 + 7)).unwrap();
    let host = Arc::new(Host(Mutex::default(), Ok(())));
    set_answer_host(Some(host.clone()));
    frame(&mut m, 51);
    assert!(answer_handler(&mut m, HESP));
    assert_eq!(
        *host.0.lock().unwrap(),
        [Request { shader: SHADER, owner: Owner::Object(handle), record: RECORD_INDEX as u16, part: 2, passes: 2 }]
    );
    // A host's refusal (the stencil mark, a target copy, a view sample)
    // leaves the handler to the guest.
    for why in ["object_stencil", "object_target", "object_view"] {
        set_answer_host(Some(Arc::new(Host(Mutex::default(), Err(why)))));
        assert!(!answer_handler(&mut m, HESP), "{why} keeps the guest's handler");
    }
    set_answer_host(None);
}
