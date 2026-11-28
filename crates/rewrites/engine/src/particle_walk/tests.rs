use super::*;
use crate::function_eval::constant::{BYTE, ZERO};
use guest_layout::memory::FlatMemory;
use crate::particle_emission::constant;

const CW: u16 = 0x027f;
const MXCSR: u32 = 0x1f80;
const TAG_TABLE: u32 = 0x004c_0000;
const TAG_DATA: u32 = 0x004c_0100;
const EVENTS: u32 = 0x004c_0200;
const SYSTEMS: u32 = 0x004c_0300;
const EMITTER_DEFS: u32 = 0x004c_1000;
const EMITTER_ARRAY: u32 = 0x004c_2000;
const PARTICLE_ARRAY: u32 = 0x004c_3000;
const SYSTEM: u32 = 0x004c_4000;
const LOCATION: u32 = 0x004c_4100;
const STATE: u32 = 0x004c_4200;
const DRAW: u32 = 0x004c_4300;
const DEF: u32 = 0x004c_4400;
const BLOBS: u32 = 0x004c_6000;
const DEFAULT: u32 = 0x004c_9000;
const HEADERS: u32 = 0x004c_a000;
const CLOCK_RECORD: u32 = 0x004c_a100;
const RECORD_TABLE: u32 = 0x004c_b000;

fn f(v: f32) -> u32 {
    v.to_bits()
}

/// A property of input `input`, the identity mapped to `lo..hi`.
fn property(m: &mut FlatMemory, at: u32, input: i16, lo: f32, hi: f32) {
    let blob = BLOBS + (at & 0xfff) * 0x10;
    m.set_u16(at, input as u16).unwrap();
    m.set_u16(at + 2, 0).unwrap();
    m.set_u16(at + 4, 0).unwrap();
    m.set_u16(at + 6, u16::MAX).unwrap();
    m.set_u32(at + 8, 1).unwrap();
    m.set_u32(at + 0xc, blob).unwrap();
    m.set_u8(blob, 0).unwrap();
    m.set_u8(blob + 1, 0).unwrap();
    m.set_u32(blob + 4, f(lo)).unwrap();
    m.set_u32(blob + 8, f(hi)).unwrap();
}

fn emitter_def(i: u32) -> u32 {
    EMITTER_DEFS + 0xb8 * i
}

fn particle_at(i: u32) -> u32 {
    PARTICLE_ARRAY + 0x40 * i
}

/// One particle: index `i`, the next `next`, its life and age.
fn particle(m: &mut FlatMemory, i: u32, next: u32, life: f32, age: f32) {
    let p = particle_at(i);
    m.set_u32(p + 4, next).unwrap();
    m.set_u32(p + 8, f(life)).unwrap();
    m.set_u32(p + 0x10, f(age)).unwrap();
    m.set_u32(p + 0x14, 0x0002_0001 + i).unwrap();
    m.set_u32(p + 0x18, 0x0004_0003).unwrap();
    for c in 0..3 {
        m.set_u32(p + 0x1c + 4 * c, f(10.0 * i as f32 + c as f32)).unwrap();
        m.set_u32(p + 0x28 + 4 * c, f(1.0 - c as f32)).unwrap();
    }
    m.set_u32(p + 0x3c, 0x00aa_bb00 | i).unwrap();
}

/// An `effe` system of two emitters: emitter 0 holds particles 3 then 1
/// (size and alpha per particle: the age and the life), emitter 1 holds
/// particle 2 (size per emitter: the system's `+4`). The definition the
/// callback draws reads inputs 0 to 2.
fn world() -> FlatMemory {
    let mut m = FlatMemory::new(0x0041_0000, 0x0012_0000);
    for (at, v) in [(ONE, 1.0), (ZERO, 0.0), (BYTE, 0.5), (constant::WORD, 1.0 / 65535.0)] {
        m.set_u32(at, f(v)).unwrap();
    }
    m.set_u32(0x0046_86cc, DEFAULT).unwrap();
    for i in 1..4 {
        m.set_u32(DEFAULT + 4 * i, f(1.0)).unwrap();
    }
    // The tag: entry 2, an effect whose event 1 holds the systems.
    m.set_u32(TAGS, TAG_TABLE).unwrap();
    m.set_u32(TAG_TABLE + 0x20, EFFE).unwrap();
    m.set_u32(TAG_TABLE + 0x28, TAG_DATA).unwrap();
    m.set_u32(TAG_DATA + 0x18, EVENTS).unwrap();
    m.set_u32(EVENTS + 0x38 + 0x34, SYSTEMS).unwrap();
    m.set_u32(SYSTEM + 0x10, 0x1234_0002).unwrap();
    m.set_u32(SYSTEM + 0x14, 1).unwrap();
    m.set_u16(SYSTEM + 0xe, 1).unwrap();
    m.set_u32(SYSTEM + 4, f(0.75)).unwrap();
    let def = SYSTEMS + 0x38;
    m.set_u32(def + 0x34, EMITTER_DEFS).unwrap();
    for i in 0..2 {
        let e = emitter_def(i);
        m.set_u32(e + 0x58 + 8, 0).unwrap();
        m.set_u32(e + 0x58 + 0xc, BLOBS + 0x800).unwrap();
        property(&mut m, e + 0x68, 0, 0.0, 1.0);
    }
    property(&mut m, emitter_def(0) + 0x48, 1, 0.0, 2.0);
    m.set_u32(emitter_def(0) + 0xb4, 0x3).unwrap();
    property(&mut m, emitter_def(1) + 0x48, 4, 0.0, 4.0);
    m.set_u32(emitter_def(1) + 0xb0, 4).unwrap();
    m.set_u32(emitter_def(1) + 0xb4, 0x13).unwrap();
    // The instances: location -> emitter 5 -> emitter 2.
    m.set_u32(EMITTERS, HEADERS).unwrap();
    m.set_u32(HEADERS + 0x44, EMITTER_ARRAY).unwrap();
    m.set_u32(PARTICLES, HEADERS + 0x80).unwrap();
    m.set_u32(HEADERS + 0x80 + 0x44, PARTICLE_ARRAY).unwrap();
    m.set_u32(LOCATION + 4, 0x0001_0005).unwrap();
    let e5 = EMITTER_ARRAY + 0x4c * 5;
    let e2 = EMITTER_ARRAY + 0x4c * 2;
    m.set_u32(e5 + 4, 3).unwrap();
    m.set_u32(e5 + 8, 2).unwrap();
    m.set_u32(e2 + 4, 0x0007_0002).unwrap();
    m.set_u32(e2 + 8, u32::MAX).unwrap();
    particle(&mut m, 3, 1, 0.25, 0.5);
    particle(&mut m, 1, u32::MAX, 0.5, 0.25);
    particle(&mut m, 2, u32::MAX, 1.0, 1.5);
    m.set_u32(DRAW, DEF).unwrap();
    m.set_u32(DEF + 0x98, 0x7).unwrap();
    m
}

fn call() -> Call {
    Call { system: SYSTEM, location: LOCATION, callback: pe::ENTRY, draw: DRAW, state: STATE }
}

fn written(w: &Walk, at: u32) -> Option<u32> {
    let b: Option<Vec<u8>> = (0..4).map(|i| w.writes.get(&(at + i)).copied()).collect();
    b.map(|b| u32::from_le_bytes(b.try_into().unwrap()))
}

#[test]
fn particles_come_in_emitter_then_list_order_with_their_records() {
    let w = walk(&world(), call(), MXCSR, CW).unwrap();
    let order: Vec<u32> = w.emitted.iter().map(|e| e.particle).collect();
    assert_eq!(order, [particle_at(3), particle_at(1), particle_at(2)]);
    let e = &w.emitted[0];
    assert_eq!(e.position, [30.0, 31.0, 32.0].map(f));
    assert_eq!(e.axis, [1.0, 0.0, -1.0].map(f));
    assert_eq!((e.words, e.packed), (0x0002_0004, 0x00aa_bb03));
}

#[test]
fn size_and_tint_are_per_particle_or_per_emitter_by_its_bits() {
    let w = walk(&world(), call(), MXCSR, CW).unwrap();
    let size: Vec<u32> = w.emitted.iter().map(|e| e.size).collect();
    assert_eq!(size, [1.0, 0.5, 3.0].map(f), "the age times 2; then the system's +4 times 4");
    let alpha: Vec<u32> = w.emitted.iter().map(|e| e.tint[0]).collect();
    assert_eq!(alpha, [0.25, 0.5, 1.0].map(f), "each particle's life");
    assert!(w.emitted.iter().all(|e| e.tint[1..] == [f(1.0); 3]), "the default white");
}

#[test]
fn the_block_takes_the_records_and_the_callbacks_inputs() {
    let w = walk(&world(), call(), MXCSR, CW).unwrap();
    assert_eq!(written(&w, STATE + 0x48), Some(SYSTEM));
    assert_eq!(written(&w, STATE + 0x4c), Some(LOCATION));
    assert_eq!(written(&w, STATE + 0x50), Some(particle_at(2)), "the last particle");
    assert_eq!(written(&w, STATE + 0x10), Some(f(0.75)), "input 4: the system's +4");
    assert_eq!(written(&w, STATE + 0x44), Some(0x17), "the emitter's 0x13 and the callback's input 2");
    let e = &w.emitted[2];
    assert_eq!(e.inputs[..2], [f(1.0), f(1.5)], "life and age");
    assert_eq!(e.inputs[2], f(3.0 * (1.0f32 / 65535.0)), "the callback's input 2: the word at +0x14");
}

#[test]
fn a_cached_input_is_read_not_made_again() {
    let mut m = world();
    m.set_u32(STATE + 0x48, SYSTEM).unwrap();
    m.set_u32(STATE + 0x44, 0x10).unwrap();
    m.set_u32(STATE + 0x10, f(0.25)).unwrap();
    let w = walk(&m, call(), MXCSR, CW).unwrap();
    assert_eq!(w.emitted[2].size, f(1.0), "the cached input 4");

    // A new system clears it.
    m.set_u32(STATE + 0x48, 0).unwrap();
    let w = walk(&m, call(), MXCSR, CW).unwrap();
    assert_eq!(w.emitted[2].size, f(3.0));
}

#[test]
fn the_system_and_location_inputs_read_their_records() {
    let mut m = world();
    m.set_u32(emitter_def(1) + 0xb4, 0x1_0780).unwrap();
    m.set_u32(LOCATION + 0x1c, f(2.5)).unwrap();
    m.set_u8(LOCATION + 3, 4).unwrap();
    m.set_u32(constant::CLOCK, CLOCK_RECORD).unwrap();
    m.set_u32(CLOCK_RECORD + 4, f(0.5)).unwrap();
    m.set_u32(CLOCK_RECORD + 8, 6).unwrap();
    m.set_u32(SYSTEM + 0x18, 0x0009_0001).unwrap();
    m.set_u32(constant::RECORDS, RECORD_TABLE).unwrap();
    m.set_u32(RECORD_TABLE + 0x44, RECORD_TABLE + 0x100).unwrap();
    m.set_u32(RECORD_TABLE + 0x100 + 0x190 + 0x6c, f(7.0)).unwrap();
    m.set_u32(RECORD_TABLE + 0x100 + 0x190 + 0x70, f(8.0)).unwrap();
    let w = walk(&m, call(), MXCSR, CW).unwrap();
    assert_eq!(written(&w, STATE + 0x1c), Some(f(2.5)), "input 7: the location's +0x1c");
    assert_eq!(written(&w, STATE + 0x20), Some(f(3.0)), "input 8: 6 ticks of 0.5");
    assert_eq!(written(&w, STATE + 0x24), Some(f(7.0)), "input 9");
    assert_eq!(written(&w, STATE + 0x28), Some(f(8.0)), "input 10");
    assert_eq!(written(&w, STATE + 0x40), Some(f(2.0)), "input 16: the location's byte 3 by BYTE");
}

#[test]
fn an_emitter_with_no_particle_is_passed_by_unevaluated() {
    let mut m = world();
    let e5 = EMITTER_ARRAY + 0x4c * 5;
    m.set_u32(e5 + 4, u32::MAX).unwrap();
    // Its size would fault.
    m.set_u32(emitter_def(0) + 0x48 + 0xc, 0x0100_0000).unwrap();
    let w = walk(&m, call(), MXCSR, CW).unwrap();
    assert_eq!(w.emitted.len(), 1);
    assert_eq!(w.emitted[0].particle, particle_at(2));
}

#[test]
fn what_the_model_does_not_take_is_refused_by_name() {
    let refused = |m: &FlatMemory, c: Call| walk(m, c, MXCSR, CW).unwrap_err().name();

    let mut m = world();
    m.set_u16(SYSTEM + 0xc, 0x400).unwrap();
    assert_eq!(refused(&m, call()), "scale");

    let mut m = world();
    m.set_u16(SYSTEMS + 0x38 + 0xc, 1).unwrap();
    assert_eq!(refused(&m, call()), "local_space");

    let mut m = nested(PRT3);
    m.set_u32(TAG_TABLE + 0x40, EFFE).unwrap();
    assert_eq!(refused(&m, child()), "no_model", "the parent's particle tag is an effect");

    let mut m = nested(PRT3);
    m.set_u32(SINGLETONS[0].1, 0).unwrap();
    assert_eq!(refused(&m, child()), "singleton_vtable");

    let mut m = nested(PRT3);
    m.set_u32(CHILD + 0x48, CHILD).unwrap();
    assert_eq!(refused(&m, child()), "depth", "its own parent");

    let m = world();
    assert_eq!(refused(&m, Call { callback: 0x0022_43c0, ..call() }), "callback");

    let mut m = world();
    m.set_u32(particle_at(1) + 4, 3).unwrap();
    assert_eq!(refused(&m, call()), "unbounded", "a cycle");

    let mut m = world();
    m.set_u32(EMITTERS, 0x0100_0000).unwrap();
    assert_eq!(refused(&m, call()), "fault");
}

#[test]
fn a_system_with_no_emitter_walks_nothing() {
    let mut m = world();
    m.set_u32(LOCATION + 4, u32::MAX).unwrap();
    m.set_u16(SYSTEM + 0xc, 0x400).unwrap();
    let w = walk(&m, call(), MXCSR, CW).unwrap();
    assert!(w.emitted.is_empty(), "the scale is never asked for");
}

#[test]
fn the_call_reads_its_four_arguments() {
    let mut m = world();
    let esp = 0x004d_0000;
    for (i, v) in [SYSTEM, LOCATION, pe::ENTRY, DRAW].into_iter().enumerate() {
        m.set_u32(esp + 4 + 4 * i as u32, v).unwrap();
    }
    assert_eq!(Call::at(&m, esp, STATE).unwrap(), call());
}

const CHILD: u32 = 0x004c_4500;
const PARTICLE_DATA: u32 = 0x004c_4600;
const CHILD_SYSTEMS: u32 = 0x004c_4700;

/// [`world`]'s emitters and particles under a child system of `group`
/// (`prt3` or `PRTM`, tag entry 3): its parent is [`world`]'s system,
/// whose definition names the particle tag entry 4 (of `group`), whose
/// systems block holds the child's definition at index 1.
fn nested(group: u32) -> FlatMemory {
    let mut m = world();
    m.set_u32(TAG_TABLE + 0x30, group).unwrap();
    m.set_u32(TAG_TABLE + 0x40, group).unwrap();
    m.set_u32(TAG_TABLE + 0x48, PARTICLE_DATA).unwrap();
    m.set_u32(CHILD + 0x10, 0x5678_0003).unwrap();
    m.set_u16(CHILD + 0xe, 1).unwrap();
    m.set_u32(CHILD + 0x48, SYSTEM).unwrap();
    m.set_u32(CHILD + 4, f(0.75)).unwrap();
    m.set_u32(SYSTEMS + 0x38 + 4, 0x9abc_0004).unwrap();
    let block = if group == PRT3 { 0x88 } else { 0x7c };
    m.set_u32(PARTICLE_DATA + block, CHILD_SYSTEMS).unwrap();
    m.set_u32(CHILD_SYSTEMS + 0x38 + 0x34, EMITTER_DEFS).unwrap();
    // The parent's own systems no longer hold the emitters.
    m.set_u32(SYSTEMS + 0x38 + 0x34, 0x0100_0000).unwrap();
    for (_, object, vtable, _) in SINGLETONS {
        m.set_u32(object, vtable).unwrap();
    }
    m
}

fn child() -> Call {
    Call { system: CHILD, ..call() }
}

#[test]
fn a_child_system_walks_its_parents_particles_systems() {
    let plain = walk(&world(), call(), MXCSR, CW).unwrap();
    for (group, singleton) in [(PRT3, SINGLETONS[0].1), (PRTM, SINGLETONS[1].1)] {
        let w = walk(&nested(group), child(), MXCSR, CW).unwrap();
        let order: Vec<u32> = w.emitted.iter().map(|e| e.particle).collect();
        assert_eq!(order, [particle_at(3), particle_at(1), particle_at(2)], "{group:#x}");
        let sizes = |w: &Walk| w.emitted.iter().map(|e| e.size).collect::<Vec<_>>();
        assert_eq!(sizes(&w), sizes(&plain), "{group:#x}: the same emitters");
        assert_eq!(written(&w, STATE + 0x48), Some(CHILD));
        assert_eq!(written(&w, singleton + 4), Some(0x9abc_0004), "{group:#x}: the singleton takes the tag");
        assert_eq!(written(&w, singleton + 8), Some(PARTICLE_DATA), "and its data");
    }
}

#[test]
fn a_childs_child_goes_through_both_parents() {
    let mut m = nested(PRT3);
    // A grandchild of PRTM tag entry 5, its parent the child: the child's
    // definition names entry 5, whose model's systems hold it at 0.
    const GRANDCHILD: u32 = 0x004c_4800;
    const MODEL_DATA: u32 = 0x004c_4900;
    const MODEL_SYSTEMS: u32 = 0x004c_4a00;
    m.set_u32(TAG_TABLE + 0x50, PRTM).unwrap();
    m.set_u32(TAG_TABLE + 0x58, MODEL_DATA).unwrap();
    m.set_u32(GRANDCHILD + 0x10, 0x0001_0005).unwrap();
    m.set_u32(GRANDCHILD + 0x48, CHILD).unwrap();
    m.set_u32(CHILD_SYSTEMS + 0x38 + 4, 0x0002_0005).unwrap();
    m.set_u32(MODEL_DATA + 0x7c, MODEL_SYSTEMS).unwrap();
    m.set_u32(MODEL_SYSTEMS + 0x34, EMITTER_DEFS).unwrap();
    let w = walk(&m, Call { system: GRANDCHILD, ..call() }, MXCSR, CW).unwrap();
    assert_eq!(w.emitted.len(), 3);
    assert_eq!(written(&w, SINGLETONS[0].1 + 4), Some(0x9abc_0004), "the child's lookup");
    assert_eq!(written(&w, SINGLETONS[1].1 + 4), Some(0x0002_0005), "then the grandchild's");
    assert_eq!(written(&w, SINGLETONS[1].1 + 8), Some(MODEL_DATA));
}
