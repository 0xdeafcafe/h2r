use std::sync::Mutex;

use engine::memory::{FlatMemory, GuestMemory};

use super::*;
use crate::effects_shadow::SAMPLE;

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
    let key = format!("layer_plan_15_effects_shadow_{name}");
    crate::native_layers::counters().into_iter().find(|(k, _)| *k == key).map_or(0, |(_, v)| v)
}

/// A BSP part drawn by a shader whose template has two layer-15 passes.
fn world() -> FlatMemory {
    let mut m = FlatMemory::new(BASE, 0x0070_0000);
    let w = |m: &mut FlatMemory, at: u32, v: u32| m.set_u32(at, v).unwrap();
    w(&mut m, engine::tags::TAG_ENTRIES_GLOBAL, TABLE);
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

/// The guest's build: the model's slot, as the arm predicts it.
fn guest_builds(m: &mut FlatMemory) -> u32 {
    let a = args(&*m, ESP).unwrap();
    let built = predict(&*m, &a).unwrap();
    m.write(a.slot, &built.slot).unwrap();
    0x1234_5600 | u32::from(built.al)
}

/// One handler on a sampled frame with the guest's two builds.
fn handler(m: &mut FlatMemory) {
    begin_handler(&*m, HESP);
    for pass in 0..2 {
        select(m, pass);
        assert_eq!(build(m, ESP), None);
        let eax = guest_builds(m);
        end_build(&*m, ESP + 4 + POP, eax);
    }
    end_handler(HESP + 8);
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
fn a_handler_the_guest_runs_as_the_model_does_matches() {
    let _t = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let mut m = world();
    frame(&mut m, SAMPLE * 3);
    let (builds, passes) = (counter("geometry_build_matched"), counter("geometry_passes_matched"));
    handler(&mut m);
    assert_eq!(counter("geometry_build_matched"), builds + 2);
    assert_eq!(counter("geometry_passes_matched"), passes + 1);
}

#[test]
fn a_slot_a_result_or_a_pass_count_the_model_did_not_make_differs() {
    let _t = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let mut m = world();
    frame(&mut m, SAMPLE * 4);
    let slot = counter("geometry_build_mismatched_slot");
    assert_eq!(build(&mut m, ESP), None);
    let eax = guest_builds(&mut m);
    m.set_u8(SLOT + 9, 7).unwrap();
    end_build(&m, ESP + 4 + POP, eax);
    assert_eq!(counter("geometry_build_mismatched_slot"), slot + 1);

    let ret = counter("geometry_build_mismatched_return");
    assert_eq!(build(&mut m, ESP), None);
    let eax = guest_builds(&mut m);
    end_build(&m, ESP + 4 + POP, eax & !0xff);
    assert_eq!(counter("geometry_build_mismatched_return"), ret + 1);

    // A handler that builds once where the template has two passes.
    let count = counter("geometry_passes_mismatched_count");
    begin_handler(&m, HESP);
    assert_eq!(build(&mut m, ESP), None);
    let eax = guest_builds(&mut m);
    end_build(&m, ESP + 4 + POP, eax);
    end_handler(HESP + 8);
    assert_eq!(counter("geometry_passes_mismatched_count"), count + 1);
}

#[test]
fn what_the_model_cannot_make_is_a_named_refusal() {
    let _t = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let mut m = world();
    frame(&mut m, SAMPLE * 5);
    m.set_u32(TABLE + 16, u32::from_be_bytes(*b"PRTM")).unwrap();
    let before = counter("geometry_refused_particle_material");
    assert_eq!(build(&mut m, ESP), None);
    assert_eq!(counter("geometry_refused_particle_material"), before + 1);
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
fn the_arm_answers_after_clean_sampled_frames_and_falls_back_at_a_difference() {
    let _t = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let mut m = world();
    for k in 1..=CLEAN {
        frame(&mut m, k * SAMPLE);
        handler(&mut m);
        assert!(!with_proof(|p| p.answering()), "not before {CLEAN} clean frames");
    }
    // The next frame's first hook closes the last sampled one.
    frame(&mut m, CLEAN * SAMPLE + 1);
    let answered = counter("answer_geometry");
    select(&mut m, 1);
    m.write(SLOT, &[0xee; 24]).unwrap();
    assert_eq!(build(&mut m, ESP), Some(true));
    assert!(with_proof(|p| p.answering()));
    assert_eq!(counter("answer_geometry"), answered + 1);
    let expect = predict(&m, &args(&m, ESP).unwrap()).unwrap();
    let mut written = [0; 24];
    m.read(SLOT, &mut written).unwrap();
    assert_eq!(written, expect.slot, "the slot is the model's, made in Rust");

    // A sampled frame runs the guest; a difference ends the answer.
    frame(&mut m, (CLEAN + 1) * SAMPLE);
    let fallback = counter("geometry_fallback");
    assert_eq!(build(&mut m, ESP), None);
    let eax = guest_builds(&mut m);
    m.set_u8(SLOT + 8, 9).unwrap();
    end_build(&m, ESP + 4 + POP, eax);
    assert_eq!(counter("geometry_fallback"), fallback + 1);
    frame(&mut m, (CLEAN + 1) * SAMPLE + 1);
    assert_eq!(build(&mut m, ESP), None, "the guest builds again");
}

#[test]
fn a_sampled_draw_takes_the_pass_its_build_made_once_and_the_material_streak_proves() {
    let _t = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let mut m = world();
    frame(&mut m, SAMPLE * 40);
    select(&mut m, 1);
    assert_eq!(build(&mut m, ESP), None);
    let pass = material_sample(&m).unwrap();
    assert_eq!((pass.shader, pass.selector, pass.imp), (SHADER, 1, 0));
    assert_eq!(material_sample(&m), None, "once");
    // An unsampled frame, or a build of another frame, is nobody's.
    assert_eq!(build(&mut m, ESP), None);
    frame(&mut m, SAMPLE * 40 + 1);
    assert_eq!(material_sample(&m), None);

    material_record(&["render_0x3b".to_owned()], &[]);
    assert!(!material_proven());
    for _ in 0..CLEAN {
        material_record(&[], &["combiner"]);
    }
    assert!(material_proven());
    let proven = counter("geometry_material_proven");
    material_record(&["stage0_address_u".to_owned()], &[]);
    assert!(!material_proven(), "a difference ends the streak");
    assert_eq!(counter("geometry_material_mismatched_stage0_address_u") > 0, true);
    assert_eq!(counter("geometry_material_proven"), proven);
}

struct Host(Mutex<Vec<Request>>, Result<(), &'static str>);

impl AnswerHost for Host {
    fn draw(&self, _: &mut dyn GuestMemory, request: &Request) -> Result<(), &'static str> {
        self.0.lock().unwrap().push(*request);
        self.1
    }
}

#[test]
fn a_proven_cluster_entry_is_drawn_by_the_host_and_a_refusal_keeps_the_guests_handler() {
    use std::sync::Arc;
    let _t = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let mut m = world();
    let record = engine::render_loop::opaque_material::RENDER_RECORDS + RECORD_INDEX * 0x20;
    m.set_u32(record + 0xc, 0x7ff << 18).unwrap();
    m.set_u32(DATA - 4, 0x0004_1020).unwrap();
    let host = Arc::new(Host(Mutex::default(), Ok(())));
    set_answer_host(Some(host.clone()));

    frame(&mut m, SAMPLE * 50 + 1);
    assert!(!answer_handler(&mut m, HESP), "nothing is proven yet");
    for k in 51..51 + CLEAN {
        frame(&mut m, k * SAMPLE);
        handler(&mut m);
    }
    for _ in 0..CLEAN {
        material_record(&[], &[]);
    }
    // A sampled frame runs the guest; the next one answers.
    frame(&mut m, (51 + CLEAN) * SAMPLE);
    assert!(!answer_handler(&mut m, HESP));
    frame(&mut m, (51 + CLEAN) * SAMPLE + 1);
    assert!(proven());
    let drawn = counter("answer_geometry_draw");
    assert!(answer_handler(&mut m, HESP));
    assert_eq!(*host.0.lock().unwrap(), [Request { shader: SHADER, cluster: 0, part: 2, passes: 2 }]);
    assert_eq!(counter("answer_geometry_draw"), drawn + 1);

    // A host that refuses leaves the handler to the guest.
    set_answer_host(Some(Arc::new(Host(Mutex::default(), Err("warming")))));
    assert!(!answer_handler(&mut m, HESP));
    // So does an entry that is not a cluster's, or a pass the model does not accept.
    set_answer_host(Some(host.clone()));
    m.set_u32(record + 0xc, 3 << 18).unwrap();
    assert!(!answer_handler(&mut m, HESP));
    m.set_u32(record + 0xc, 0x7ff << 18).unwrap();
    m.set_u32(0x0064_0000 + 4, 0).unwrap();
    assert!(!answer_handler(&mut m, HESP), "no vertex class to draw");
    assert_eq!(host.0.lock().unwrap().len(), 1);
    set_answer_host(None);
    material_record(&["render_0x3b".to_owned()], &[]);
}
