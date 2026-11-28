//! Invented script tables only: no disc, no guest.
use super::*;
use hs::fixture::{Builtin, Exe, Tables};
use hs::vm::mock::{Policy, RecordingEngine};
use hs::vm::{Cell, EngineCall, EngineError, VmImage};
use hs::{Catalog, Scenario};
use sim_world::World;
use std::cell::RefCell;
use std::rc::Rc;

const VOID: u16 = 4;
const SHORT: u16 = 7;
/// The invented engine-facing builtin: `(log <short>)`.
const LOG: u16 = 30;

/// A startup script that calls `(log 1)`, and a continuous one `(log 2)`.
fn world_of_scripts() -> (Scenario, Catalog) {
    let mut exe = Exe::default();
    exe.builtins.resize(LOG as usize + 1, Builtin::default());
    exe.builtins[LOG as usize] = Builtin {
        returns: VOID,
        flags: 0,
        parameters: vec![SHORT],
        usage: None,
    };
    let mut catalog = exe.catalog();
    let mut t = Tables::new();
    let one = t.value(SHORT, 1);
    let call = t.call(LOG, "log", VOID, &[one]);
    t.script("start", 0, VOID, call);
    let two = t.value(SHORT, 2);
    let call = t.call(LOG, "log", VOID, &[two]);
    t.script("tick", 2, VOID, call);
    let scenario = t.scenario();
    catalog.observe(&scenario).expect("the invented tables are valid");
    (scenario, catalog)
}

fn new_shared() -> Shared {
    Shared::default()
}

#[test]
fn a_rust_body_replaces_the_engine_call_by_name() {
    let (scenario, catalog) = world_of_scripts();
    let image = VmImage::without_stubs();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let mut rt = ScriptRuntime::new(&scenario, &catalog, &image, new_shared()).unwrap();
    let sink = seen.clone();
    let missing = Natives::new()
        .with(Native::new("log", move |_, call: EngineCall<'_>| {
            sink.borrow_mut().push(call.arguments[0].0 as i16);
            Ok(Cell::ZERO)
        }))
        .with(Native::new("no_such_builtin", |_, _| Err(EngineError::Unimplemented)))
        .bind(&mut rt.vm, &catalog);
    assert_eq!(missing, ["no_such_builtin"]);

    let mut engine = RecordingEngine::new(Policy::Neutral);
    rt.initialize(&mut engine).unwrap();
    rt.update(&mut engine).unwrap();
    assert_eq!(*seen.borrow(), [1, 2]);
    assert!(engine.counts.is_empty(), "the engine adapter was not called");
    assert!(rt.take_faults().is_empty());
}

#[test]
fn a_builtin_without_a_body_goes_to_the_engine() {
    let (scenario, catalog) = world_of_scripts();
    let image = VmImage::without_stubs();
    let mut rt = ScriptRuntime::new(&scenario, &catalog, &image, new_shared()).unwrap();
    let mut engine = RecordingEngine::new(Policy::Neutral);
    rt.initialize(&mut engine).unwrap();
    rt.update(&mut engine).unwrap();
    assert_eq!(engine.counts[&LOG], 2);
}

#[test]
fn the_world_saves_and_restores_the_scripts_section() {
    let (scenario, catalog) = world_of_scripts();
    let image = VmImage::without_stubs();

    let shared = new_shared();
    let mut world = World::new(7);
    world.attach(Box::new(ScriptStore(shared.clone()))).unwrap();
    let before_start = world.sim_hash();
    let mut rt = ScriptRuntime::new(&scenario, &catalog, &image, shared).unwrap();
    let mut engine = RecordingEngine::new(Policy::Neutral);
    rt.initialize(&mut engine).unwrap();
    rt.update(&mut engine).unwrap();
    assert_ne!(world.sim_hash(), before_start, "the threads are in the hash");
    let saved = world.save();

    // A second world takes the save; its runtime carries on from it.
    let other = new_shared();
    let mut world2 = World::new(7);
    world2.attach(Box::new(ScriptStore(other.clone()))).unwrap();
    let mut rt2 = ScriptRuntime::new(&scenario, &catalog, &image, other.clone()).unwrap();
    world2.restore_into(&saved).unwrap();
    assert_eq!(world2.sim_hash(), world.sim_hash());
    assert_eq!(world2.save(), saved);

    let mut e1 = RecordingEngine::new(Policy::Neutral);
    let mut e2 = RecordingEngine::new(Policy::Neutral);
    e1.time = 1;
    e2.time = 1;
    rt.update(&mut e1).unwrap();
    rt2.update(&mut e2).unwrap();
    assert_eq!(e1.counts, e2.counts);
    assert_eq!(world.save(), world2.save(), "restored scripts run identically");
}

#[test]
fn a_damaged_section_is_a_typed_error() {
    let shared = new_shared();
    let mut world = World::new(1);
    world.attach(Box::new(ScriptStore(shared))).unwrap();
    let mut saved = world.save();
    saved.pop();
    assert!(world.restore_into(&saved).is_err());
}
