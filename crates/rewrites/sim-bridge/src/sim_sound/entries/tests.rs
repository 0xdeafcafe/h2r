use super::*;
use engine::entry::{self, EntryFuture, Regs};
use engine::testing::FakeGuest;
use hooks::{Base, Registry, Switches};
use sim_sound::instances::{Flat, Permutation, PitchRange, Request, Scale, SoundTag, SourceParams, TagTable};
use sim_sound::looping::{LoopTag, Segment, TrackTag};
use sim_sound::owners::{NullOwners, Own, Owner};
use sim_sound::starts::{LOOP_GROUP, OWNER_BIT, SOUND_GROUP, SOUND_TABLE, STARTED_TABLE, StartOf};

use crate::sim_sound::request;

const STACK: u32 = 0x0030_0000;
const TIME: u32 = 0x0052_0000;
const SEEDS: u32 = 0x0052_0100;
const GLOBALS: u32 = TIME + 0x2800;
const RECORD: u32 = 0x0060_0000;
/// The map's tag entries and tag data ([`guest`], [`with_effect`]).
const TAGS: u32 = 0x0062_0000;
const SOUND_DEF: u32 = TAGS + 0x100;
const GESTALT_AT: u32 = TAGS + 0x200;
const BANKS: u32 = TAGS + 0x300;
const LIST_DEF: u32 = TAGS + 0x400;
const LIST_ROWS: u32 = TAGS + 0x500;
const SCENARIO_AT: u32 = TAGS + 0x800;
const TAG: TagIndex = TagIndex(0xe100_0001);

/// Levels that name their source as not ready.
struct Refusing;

impl Levels for Refusing {
    fn level(&self, _: TagIndex, _: &SourceParams) -> f32 {
        f32::NAN
    }
    fn refused(&self) -> Option<&'static str> {
        Some("test")
    }
}

fn tags() -> TagTable {
    let mut t = TagTable::default();
    t.sounds.insert(TAG, SoundTag { range_count: 1, promotion: 0xff, ..SoundTag::default() });
    t.ranges.push(PitchRange { natural: [0, 0], playable: [0, 0], first_permutation: 0, count: 1 });
    t.permutations.push(Permutation { skip: 0, length_ms: Some(100) });
    t
}

fn table(levels: Arc<dyn Levels + Send + Sync>) -> Shared {
    Arc::new(Mutex::new(Table::new(Arc::new(tags()), levels)))
}

/// A guest with the tick, the seed word, the sound globals (sound on) and
/// an empty objects array.
fn guest(tick: u32) -> FakeGuest {
    let mut g = FakeGuest::new();
    g.map(0x004e_0000, 0x1_0000).map(STACK - 0x8000, 0x1_0000).map(TIME, 0x4000).map(RECORD, 0x1000);
    g.put_u32(points::GAME_TIME.base, TIME).put_u32(TIME + 8, tick);
    g.put_u32(points::RANDOM_SEEDS.base, SEEDS).put_u32(SEEDS + 4, 0x1234_5678);
    g.put_u32(points::SOUND_GLOBALS.base, GLOBALS);
    for o in sg::ON {
        g.put_u8(GLOBALS + o, 1);
    }
    g.put_u32(points::OBJECTS.base, TIME + 0x2000);
    // The map's tags: tag 1 ([`TAG`]) of no group yet, its definition with
    // bank 0 and no promotion.
    g.map(TAGS, 0x1000).put_u32(points::TAG_ENTRIES.base, TAGS).put_u32(TAGS + 16 + 8, SOUND_DEF);
    g.put_u8(SOUND_DEF + 0xc, 0xff).put_u8(SOUND_DEF + 0xd, 0);
    g
}

fn regs() -> Regs {
    Regs { esp: STACK, ..Default::default() }
}

fn entry_of(body: Body) -> &'static Entry {
    ENTRIES.iter().find(|e| e.body == body).unwrap()
}

fn run_body(
    t: &Shared,
    body: Body,
    g: FakeGuest,
    regs: Regs,
    callee: impl FnMut(&GuestCall, &mut dyn GuestMemory) -> Regs,
) -> entry::testing::Run {
    let t = Arc::clone(t);
    let e = entry_of(body);
    let f = move |cx: Cx| -> EntryFuture { Box::pin(run(cx, Arc::clone(&t), e, Arc::new(AtomicBool::new(false)))) };
    entry::testing::run(&f, g, regs, callee)
}

fn run_phase(t: &Shared, g: FakeGuest) -> entry::testing::Run {
    let t = Arc::clone(t);
    let f = move |cx: Cx| -> EntryFuture {
        let t = Arc::clone(&t);
        Box::pin(async move { phase(&cx, &t).await })
    };
    entry::testing::run(&f, g, regs(), |_, _| Regs::default())
}

/// The addresses in the first column of the doc's "Entries" table.
fn doc_entries() -> Vec<u32> {
    let doc = include_str!("../../../../../../docs/engine/sound-instances.md");
    let start = doc.find("\n## Entries").expect("the Entries section");
    let section = &doc[start..];
    let end = section.find("\nTag readers").expect("the end of the table");
    let mut out = Vec::new();
    for row in section[..end].lines().filter(|l| l.starts_with("| `0x")) {
        let first = row.split('|').nth(1).unwrap();
        for word in first.split('`').filter(|w| w.starts_with("0x")) {
            out.push(u32::from_str_radix(word.trim_start_matches("0x").split_whitespace().next().unwrap(), 16).unwrap());
        }
    }
    out
}

#[test]
fn every_outside_entry_has_a_rust_body() {
    let doc = doc_entries();
    assert!(doc.len() >= 20, "{doc:x?}");
    for addr in &doc {
        assert!(body(*addr).is_some(), "{addr:#x} has no body");
    }
    let mut ours: Vec<u32> = ENTRIES.iter().map(|e| e.point.base).collect();
    let mut want = doc.clone();
    ours.sort();
    want.sort();
    assert_eq!(ours, want, "ENTRIES is the doc's table");
    // Every entry is hooked by the table on every run, the defaults and
    // a spec that turns everything it can off alike.
    for spec in ["", "-impl.sim"] {
        let mut r = Registry::new(Switches::parse(spec).unwrap());
        r.add(&SoundTable::new(table(Arc::new(Refusing)))).unwrap();
        let plan = r.build(&Base).unwrap();
        for e in ENTRIES {
            let chain = plan.chains.get(&e.point.base);
            let hooked = chain.is_some_and(|c| {
                c.implement.as_ref().is_some_and(|h| h.label == "sound table")
                    || c.before.iter().any(|h| h.label == "sound table")
            });
            assert!(hooked, "{spec}: {}", e.point.name);
        }
    }
}

/// The table's sounds are the world's sound section: a save carries them
/// and a restore into another world puts them in that world's table.
#[test]
fn the_world_saves_the_tables_sounds_and_restores_them_into_its_table() {
    let a = table(Arc::new(Refusing));
    let request = Request { selection: Some((0, 0)), level: 10.0, ..Request::new(TAG) };
    lock(&a).sounds.admit(&mut Rng::new(1), &tags(), request, 0).unwrap();
    let saved = crate::world::with_table(Arc::clone(&a)).save();
    let b = table(Arc::new(Refusing));
    let mut world = crate::world::with_table(Arc::clone(&b));
    assert_ne!(world.save(), saved, "an empty table saves another section");
    world.restore_into(&saved).unwrap();
    assert!(!lock(&b).sounds.instances().is_empty());
    assert_eq!(lock(&b).sounds.encode(), lock(&a).sounds.encode());
    assert_eq!(world.save(), saved);
}

#[test]
fn create_and_drop_are_the_lifecycle_and_reset_the_table() {
    // The guest's create and dispose are not called: the bodies are
    // `lifecycle`'s (its tests cover the globals). With the count above 1
    // both only count, and each leaves a fresh table.
    assert!(!Body::Create.calls_original() && !Body::Drop.calls_original());
    for body in [Body::Create, Body::Drop] {
        let t = table(Arc::new(Refusing));
        lock(&t).sounds.admit(&mut Rng::new(1), &tags(), Request { selection: Some((0, 0)), level: 10.0, ..Request::new(TAG) }, 0).unwrap();
        let mut g = guest(0);
        g.put_u32(crate::sim_sound::lifecycle::REFCOUNT, 2);
        let r = run_body(&t, body, g, regs(), |c, _| panic!("{body:?} called {:#x}", c.target));
        assert_eq!(r.result, Ok(Ret::new(0, 0)));
        assert!(r.calls.is_empty());
        assert!(lock(&t).sounds.instances().is_empty(), "{body:?}: a fresh table");
    }
}

#[test]
fn the_sound_phase_runs_once_per_tick_number() {
    let t = table(Arc::new(Flat(10.0)));
    let h = lock(&t).sounds.admit(&mut Rng::new(1), &tags(), Request { selection: Some((0, 0)), level: 10.0, ..Request::new(TAG) }, 0).unwrap();
    let r = run_phase(&t, guest(1));
    assert_eq!(r.result, Ok(Ret::new(0, 0)));
    run_phase(&t, guest(1));
    assert_eq!(lock(&t).ledger.phase_runs, 1, "the same tick number steps once");
    assert!(lock(&t).sounds.live(h));
    // 100 ms is three ticks: it plays out on tick 3.
    run_phase(&t, guest(3));
    let t = lock(&t);
    assert_eq!(t.ledger.phase_runs, 2);
    assert!(!t.sounds.live(h));
    assert_eq!(t.sounds.counters().ended(Reason::PlayedOut), 1);
    assert_eq!(t.counters()["sound.ended.played_out"], 1);
    assert_eq!(t.outbox.len(), 2, "the start and the end wait for the service");
}

#[test]
fn a_units_line_end_clears_its_line_slot_with_no_guest_call() {
    // Object 5, salt 1, a biped; its dialogue state at +0x100.
    const UNIT: u32 = 0x0001_0005;
    let (objects, headers, object, dialogue) = (TIME + 0x2000, TIME + 0x3000, TIME + 0x3400, TIME + 0x3800);
    let t = table(Arc::new(Flat(10.0)));
    let h = lock(&t).sounds.admit(&mut Rng::new(7), &tags(), Request { owner: Owner::Unit(UNIT), selection: Some((0, 0)), level: 10.0, ..Request::new(TAG) }, 0).unwrap();
    let mut g = guest(3);
    g.put_u32(objects + 0x38, 16).put_u32(objects + 0x24, 12).put_u32(objects + 0x44, headers);
    g.put_u16(headers + 5 * 12, 1).put_u32(headers + 5 * 12 + 8, object);
    g.put_u16(object + 0x33e, 0x100).put_u32(object + 0x100 + 0x38, TAG.0);
    g.put_u32(units::DIALOGUE, dialogue).put_u8(dialogue + units::DIALOGUE_ON, 1);
    let t2 = Arc::clone(&t);
    let f = move |cx: Cx| -> EntryFuture {
        let t = Arc::clone(&t2);
        Box::pin(async move { phase(&cx, &t).await })
    };
    let r = entry::testing::run(&f, g, regs(), |c, _| panic!("no guest call: {:#x}", c.target));
    assert_eq!(r.result, Ok(Ret::new(0, 0)));
    assert!(!lock(&t).sounds.live(h), "100 ms plays out on tick 3");
    assert_eq!(r.guest.u32(object + 0x100 + 0x38), Ok(NONE), "the unit forgot its line");
}

#[test]
fn the_hash_is_the_same_with_any_host() {
    // Object 5 is live: index 5 of a 16-row array, salt 1.
    const UNIT: u32 = 0x0001_0005;
    const PERMS: u32 = TIME + 0x3c00;
    let t = table(Arc::new(Flat(10.0)));
    let mut plain = Sounds::new();
    for s in [&mut lock(&t).sounds, &mut plain] {
        for owner in [Owner::None, Owner::Unit(UNIT)] {
            s.admit(&mut Rng::new(7), &tags(), Request { owner, selection: Some((0, 0)), level: 10.0, ..Request::new(TAG) }, 0).unwrap();
        }
    }
    for tick in 1..5 {
        let mut g = guest(tick);
        let objects = TIME + 0x2000;
        g.put_u32(objects + 0x38, 16).put_u32(objects + 0x24, 12).put_u32(objects + 0x44, TIME + 0x3000);
        g.put_u16(TIME + 0x3000 + 5 * 12, 1);
        // The tag's first range is 0, its permutation row at PERMS.
        g.put_u32(points::TAG_ENTRIES.base, TIME + 0x3800).put_u32(TIME + 0x3800 + 16 + 8, TIME + 0x3900);
        g.put_u32(GESTALT, TIME + 0x3a00).put_u32(TIME + 0x3a00 + 0x24, TIME + 0x3b00).put_u32(TIME + 0x3a00 + 0x2c, PERMS);
        // The unit's header names no object data, so the native mouth
        // stops at its unit test and asks the guest nothing.
        let r = run_phase(&t, g);
        assert!(r.calls.iter().all(|c| c.target != UNIT_MOUTH && c.target != crate::sim_sound::mouth::CACHE_TOUCH));
        plain.step(tick, &mut NullOwners);
    }
    // The guest host and the null one end the same sounds on the same
    // ticks: the table hashes the same.
    assert_eq!(lock(&t).sounds.section_hash(), plain.section_hash());
    assert!(lock(&t).sounds.instances().is_empty());
    assert_eq!(lock(&t).ledger.mouth_calls, 2, "ticks 1 and 2 made the unit's mouth");
    // The service hands the events over and changes nothing hashed.
    let before = lock(&t).sounds.section_hash();
    run_body(&t, Body::Service, guest(5), regs(), |_, _| Regs::default());
    assert_eq!(lock(&t).sounds.section_hash(), before);
    assert!(lock(&t).ledger.service_events > 0);
}

/// What the host hears, in order.
struct Recorder(Arc<Mutex<Vec<String>>>);

impl TableHost for Recorder {
    fn event(&mut self, e: &TableEvent) {
        let kind = match e {
            TableEvent::Started { .. } => "start",
            TableEvent::Stopped { .. } => "stop",
            TableEvent::Ended { .. } => "end",
        };
        self.0.lock().unwrap().push(format!("{kind} {:#x}", e.handle().raw()));
    }
    fn frame(&mut self, s: &Scene) {
        self.0.lock().unwrap().push(format!("frame {} {}", s.tick, s.sources.len()));
    }
}

#[test]
fn the_service_hands_the_host_the_events_in_the_tables_order_then_the_scene() {
    let t = table(Arc::new(Flat(10.0)));
    let log = Arc::new(Mutex::new(Vec::new()));
    lock(&t).set_host(Box::new(Recorder(Arc::clone(&log))));
    let request = Request { selection: Some((0, 0)), level: 10.0, ..Request::new(TAG) };
    // 100 ms: plays out on tick 3.
    let a = lock(&t).sounds.admit(&mut Rng::new(1), &tags(), request, 0).unwrap();
    run_phase(&t, guest(1));
    let b = lock(&t).sounds.admit(&mut Rng::new(2), &tags(), request, 2).unwrap();
    assert!(lock(&t).sounds.stop(b, 2, 3, &mut NullOwners));
    run_phase(&t, guest(3));
    assert!(log.lock().unwrap().is_empty(), "the phase hands nothing over");
    run_body(&t, Body::Service, guest(3), regs(), |_, _| Regs::default());
    let (a, b) = (a.raw(), b.raw());
    assert_eq!(
        *log.lock().unwrap(),
        [format!("start {a:#x}"), format!("start {b:#x}"), format!("stop {b:#x}"), format!("end {a:#x}"), "frame 3 1".to_owned()],
        "the tick's calls, then the ends, then the scene with the one sound left",
    );
}

/// The record the start reads: explicit selection (flag bit 10).
fn record(g: &mut FakeGuest) {
    g.put_u32(RECORD, 1 << 10).put_u16(RECORD + 0x9c, 0).put_u16(RECORD + 0x9e, 0);
}

fn start(levels: Arc<dyn Levels + Send + Sync>, open: bool) -> (Shared, entry::testing::Run) {
    let t = table(levels);
    let mut g = with_tag(guest(4));
    record(&mut g);
    with_gate(&mut g, open);
    let r = run_body(&t, Body::StartRecord, g, Regs { eax: RECORD, ..regs() }, no_calls);
    (t, r)
}

/// No guest call: the gate and the rule run in Rust.
fn no_calls(c: &GuestCall, _: &mut dyn GuestMemory) -> Regs {
    panic!("unexpected call {:#x}", c.target)
}

const GATE_DEFAULTS: u32 = TAGS + 0xc00;
const GATE_SCALES: u32 = TAGS + 0xd00;
const GATE_ROWS: u32 = TAGS + 0xe00;
const GATE_GLOBALS: u32 = TAGS + 0xf00;
const GATE_CLASSES: u32 = TAGS + 0x1000;
const GATE_STATES: u32 = TAGS + 0x1200;
const GATE_USERS: u32 = TAGS + 0x1300;

/// What `0x126c30` reads for [`TAG`] (`crate::sim_sound::gate`), and the
/// image constants it reads (test values). Open: the class plays, a chance
/// of -1 never skips, 0 dB at the first mapped user (3), over a floor of
/// -64. Closed: the class does not play.
fn with_gate(g: &mut FakeGuest, open: bool) {
    use crate::sim_sound::gate::{CLOSE, FLOOR, ONE, SEED_SCALE, SHORTEST};
    g.put_f32(FLOOR, -64.0).put_f32(SHORTEST, 0.001).put_f32(ONE, 1.0).put_f32(CLOSE, 0.0625).put_f32(SEED_SCALE, 1.0 / 65536.0);
    g.put_u8(SOUND_DEF + 5, u8::from(open)).put_u8(SOUND_DEF + 0xa, 1);
    g.put_u32(GESTALT, GESTALT_AT).put_u32(GESTALT_AT + 4, GATE_DEFAULTS).put_u32(GESTALT_AT + 0xc, GATE_SCALES);
    g.put_u32(GESTALT_AT + 0x24, GATE_ROWS).put_u16(GATE_ROWS + 0xa, 1);
    g.put_f32(GATE_DEFAULTS, 1.0).put_f32(GATE_DEFAULTS + 4, 10.0).put_f32(GATE_DEFAULTS + 8, -1.0);
    g.put_f32(GATE_SCALES + 0xc, 1.0).put_f32(GATE_SCALES + 0x10, 1.0);
    // Class 0: the globals' sound classes are tag 5's; its runtime state
    // takes no ducking.
    g.put_u32(points::GLOBALS_TAG.base, GATE_GLOBALS).put_u32(GATE_GLOBALS + 0xc0, 1).put_u32(GATE_GLOBALS + 0xc4, GATE_GLOBALS + 0x40);
    g.put_u32(GATE_GLOBALS + 0x44, 5).put_u32(TAGS + 5 * 16 + 8, GATE_CLASSES).put_u32(GATE_CLASSES + 4, GATE_CLASSES + 0x100);
    g.map(GATE_CLASSES + 0x100, 0x100);
    g.put_u32(crate::sim_sound::levels::CLASS_STATES, GATE_STATES).put_u8(GATE_STATES + 0xc, 1);
    g.put_u32(points::PLAYER_GLOBALS.base, GATE_USERS);
    for n in 0..3 {
        g.put_u32(GATE_USERS + 0xc + 4 * n, NONE);
    }
    g.put_u32(GATE_USERS + 0x18, 0);
}

/// The record the last component admission built (its flags and `+0x58`).
fn component_record(r: &entry::testing::Run) -> (u32, u32) {
    let record = STACK - start_layer::FRAME;
    (r.guest.u32(record).unwrap(), r.guest.u32(record + 0x58).unwrap())
}

fn with_tag(mut g: FakeGuest) -> FakeGuest {
    g.put_u32(STACK + 4, TAG.0);
    g
}

#[test]
fn a_start_record_is_admitted_into_the_table() {
    let t = table(Arc::new(Flat(10.0)));
    let mut g = with_tag(guest(4));
    record(&mut g);
    with_gate(&mut g, true);
    // The sound's tag is of no group: no effect, no start row.
    let r = run_body(&t, Body::StartRecord, g, Regs { eax: RECORD, ..regs() }, no_calls);
    let handle = r.result.unwrap().eax;
    assert_ne!(handle, NONE);
    let t = lock(&t);
    let i = t.sounds.get(Handle::from_raw(handle)).unwrap();
    assert_eq!((i.tag, i.start, i.options.channel), (TAG, 4, 3));
    assert_eq!(t.ledger.draws, 2, "the gain and pitch draws");
    assert_ne!(r.guest.u32(SEEDS + 4), Ok(0x1234_5678), "the seed word moved");
    assert_eq!(t.ledger.entry_guest_calls, 0, "the gate is Rust");
    assert!(t.sounds.starts().is_empty());
}

#[test]
fn a_start_in_its_promotions_cooldown_is_refused_by_the_rule_in_rust() {
    let t = table(Arc::new(Flat(10.0)));
    let mut g = with_tag(guest(4));
    record(&mut g);
    // Promotion 0: one enabled rule, threshold 5, active and cooling down
    // to clock 50; the clock is 10.
    let (p, rules, counters) = (TAGS + 0x900, TAGS + 0xa00, TAGS + 0xb00);
    g.put_u8(SOUND_DEF + 0xc, 0).put_u32(GESTALT, GESTALT_AT).put_u32(GESTALT_AT + 0x4c, p);
    g.put_u32(p, 1).put_u32(p + 4, rules).put_u32(p + 8, 1).put_u32(p + 0xc, counters);
    g.put_u32(p + 0x10, 0).put_u32(p + 0x18, 50);
    g.put_u16(rules + 2, 1).put_u32(rules + 8, 5);
    g.put_u32(GLOBALS + 0x84, 10);
    with_gate(&mut g, true);
    let r = run_body(&t, Body::StartRecord, g, Regs { eax: RECORD, ..regs() }, no_calls);
    assert_eq!(r.result.unwrap().eax, NONE);
    assert!(lock(&t).sounds.instances().is_empty());
    assert_eq!(r.guest.u32(p + 0x14), Ok(10), "the rule kept the clock it saw");
    assert_eq!(lock(&t).ledger.refused_unreadable, 0);
}

#[test]
fn a_start_refuses_by_name_when_its_levels_do() {
    let (t, r) = start(Arc::new(Refusing), true);
    assert_eq!(r.result.unwrap().eax, NONE);
    let t = lock(&t);
    assert_eq!(t.ledger.refused_no_level, 1);
    assert_eq!(t.ledger.draws, 0, "a refusal takes no draw");
    assert!(t.sounds.instances().is_empty());
    // A closed gate admits nothing and calls no rule.
    let (t, r) = start(Arc::new(Flat(10.0)), false);
    assert_eq!(r.result.unwrap().eax, NONE);
    assert!(r.calls.is_empty());
    assert!(lock(&t).sounds.instances().is_empty());
}

#[test]
fn a_stop_fades_the_sound_and_stop_all_clears_the_table() {
    let t = table(Arc::new(Flat(10.0)));
    let h = lock(&t).sounds.admit(&mut Rng::new(1), &tags(), Request { selection: Some((0, 0)), level: 10.0, ..Request::new(TAG) }, 0).unwrap();
    lock(&t).sounds.admit(&mut Rng::new(2), &tags(), Request { selection: Some((0, 0)), level: 10.0, ..Request::new(TAG) }, 0).unwrap();
    let r = run_body(&t, Body::Stop, guest(1), Regs { ebx: h.raw(), ..regs() }, |_, _| Regs::default());
    assert_eq!(r.result, Ok(Ret::new(0, 0)));
    assert!(matches!(lock(&t).sounds.get(h).unwrap().phase, sim_sound::owners::Phase::Fading { since: 1, .. }));
    run_body(&t, Body::StopAll, guest(1), regs(), |_, _| Regs::default());
    let t = lock(&t);
    assert!(t.sounds.instances().is_empty());
    assert_eq!(t.sounds.counters().ended(Reason::Cleared), 2);
}

#[test]
fn a_refused_entry_is_counted_by_its_address() {
    let t = table(Arc::new(Flat(10.0)));
    lock(&t).refuse(0x21d390);
    let c = lock(&t).counters();
    assert_eq!(c["sound.refused_entry"], 1);
    assert_eq!(c["sound.refused_entry.0x21d390"], 1);
    // No body refuses today; one that did would not also call its original.
    for e in ENTRIES {
        assert_eq!(e.body.refusal(), None, "{}", e.point.name);
    }
    assert!(REFUSING.iter().all(|(b, _)| !b.calls_original()));
}

#[test]
fn the_environment_and_the_first_listener_are_the_guests_words() {
    let t = table(Arc::new(Flat(10.0)));
    let mut g = guest(0);
    g.put_u32(GLOBALS + sg::ENV, 7).put_u32(GLOBALS + sg::ENV_NEXT, 9);
    let r = run_body(&t, Body::Environment, g, Regs { ecx: 8, ..regs() }, |_, _| Regs::default());
    assert_eq!((r.guest.u32(GLOBALS + sg::ENV), r.guest.u32(GLOBALS + sg::ENV_NEXT)), (Ok(8), Ok(7)));
    // User 1 is the first mapped; its listener is active.
    let mut g = guest(0);
    let users = TIME + 0x1000;
    g.put_u32(points::PLAYER_GLOBALS.base, users);
    for u in 0..4 {
        g.put_u32(users + 0xc + 4 * u, if u == 1 { 0 } else { NONE });
    }
    let listener = GLOBALS + sg::LISTENERS + sg::LISTENER;
    g.put_u8(listener + sg::LISTENER_ACTIVE, 1);
    let r = run_body(&t, Body::FirstListener, g, regs(), |_, _| Regs::default());
    assert_eq!(r.result, Ok(Ret::new(listener, 0)));
}

#[test]
fn a_pause_records_the_kernel_tick_count_with_no_guest_call() {
    let t = table(Arc::new(Flat(10.0)));
    let mut g = guest(0);
    let ticks = TIME + 0x3000;
    g.put_u32(crate::sim_sound::clock::TICK_COUNT_IMPORT, ticks).put_u32(ticks, 98_765).put_u8(GLOBALS + sg::PAUSED, 1);
    let r = run_body(&t, Body::Paused, g, Regs { eax: 1, ..regs() }, |c, _| panic!("no guest call: {:#x}", c.target));
    assert_eq!(r.result, Ok(Ret::new(0, 0)));
    assert_eq!(r.guest.u8(GLOBALS + sg::PAUSED), Ok(0));
    assert_eq!(r.guest.u32(GLOBALS + sg::PAUSE_TIME), Ok(98_765));
    assert_eq!(r.guest.u8(GLOBALS + sg::BUSY), Ok(0));
}

#[test]
fn the_range_word_is_read_only_for_a_class_with_flag_bit_5() {
    let t = table(Arc::new(Flat(10.0)));
    // The tag's class 0 row in the sound globals tag's classes.
    let (tag, range, classes) = (TIME + 0x3000, TIME + 0x3100, TIME + 0x3200);
    let mut g = guest(0);
    g.put_u32(points::GLOBALS_TAG.base, TIME + 0x3300).put_u32(TIME + 0x3300 + 0xc0, 1).put_u32(TIME + 0x3300 + 0xc4, TIME + 0x3400);
    g.put_u32(TIME + 0x3404, 2).put_u32(points::TAG_ENTRIES.base, TIME + 0x3500).put_u32(TIME + 0x3500 + 2 * 16 + 8, TIME + 0x3600);
    g.put_u32(TIME + 0x3604, classes).put_u16(range + 6, 12);
    let mut stack = g.clone();
    stack.put_u32(STACK + 4, range).put_u32(STACK + 8, tag);
    let r = run_body(&t, Body::HistoryLast, stack.clone(), regs(), |_, _| panic!("no guest call"));
    assert_eq!(r.result, Ok(Ret::new(NONE, 8)), "flag bit 5 clear");
    stack.put_u8(classes + 0xa, 1 << 5);
    let r = run_body(&t, Body::HistoryLast, stack, regs(), |_, _| panic!("no guest call"));
    assert_eq!(r.result, Ok(Ret::new(12, 8)));
    let _ = tag;
}

const LOOP_TAG: TagIndex = TagIndex(0xe200_0002);
const SOURCE: u32 = RECORD + 0x400;

/// One track whose loop segment is [`TAG`].
fn loop_table() -> Shared {
    let mut t = tags();
    t.scales.push(Scale { pitch: [0, 0] });
    let mut segments = [None; Segment::COUNT];
    segments[usize::from(Segment::LOOP.0)] = Some(TAG);
    let track = TrackTag { flags: 0, segments, fade_in: 0.0, crossfade: 0.0, fade_out: 0.1 };
    t.loops.insert(LOOP_TAG, LoopTag { tracks: vec![track], details: vec![] });
    Arc::new(Mutex::new(Table::new(Arc::new(t), Arc::new(Flat(10.0)))))
}

/// `0x21a250`'s eight arguments on the stack, the source record at [`SOURCE`].
fn loop_call(tick: u32, tag: TagIndex, owner: u32, mode: u32, flags: u32, fade: f32, word5: u32) -> FakeGuest {
    let mut g = guest(tick);
    g.put_u32(SOURCE + 20, word5);
    for (i, word) in [tag.0, 0, owner, 0, SOURCE, mode, flags, fade.to_bits()].into_iter().enumerate() {
        g.put_u32(STACK + 4 + 4 * i as u32, word);
    }
    g
}

fn loop_update(t: &Shared, g: FakeGuest) -> Ret {
    run_body(t, Body::LoopUpdate, g, regs(), |_, _| panic!("the loop update calls nothing")).result.unwrap()
}

#[test]
fn a_loop_update_starts_keeps_and_stops_the_loop_of_an_owner() {
    let t = loop_table();
    assert_eq!(loop_update(&t, loop_call(0, LOOP_TAG, 0x55, 0, 0, 0.0, 1)), Ret::new(0, 32), "started");
    let h = lock(&t).sounds.loop_find(LOOP_TAG, 0x55).expect("the owner's loop");
    let primary = lock(&t).sounds.loops().get(h).unwrap().tracks[0].primary.unwrap();
    assert_eq!(lock(&t).sounds.get(primary).unwrap().tag, TAG);
    // Kept: the same loop, its new source.
    assert_eq!(loop_update(&t, loop_call(1, LOOP_TAG, 0x55, 1, 0, 0.0, 2)), Ret::new(0, 32));
    assert_eq!(lock(&t).sounds.loops().len(), 1);
    assert_eq!(lock(&t).sounds.loops().get(h).unwrap().source.0[5], 2);
    // Another owner has none to stop.
    assert_eq!(loop_update(&t, loop_call(1, LOOP_TAG, 0x56, 2, 0, 0.0, 2)), Ret::new(1, 32));
    // The stop fades it over the given fade; a second stop does nothing.
    assert_eq!(loop_update(&t, loop_call(2, LOOP_TAG, 0x55, 2, 0, 0.5, 2)), Ret::new(0, 32));
    assert!(lock(&t).sounds.loops().get(h).unwrap().stopping);
    assert!(matches!(lock(&t).sounds.get(primary).unwrap().phase, sim_sound::owners::Phase::Fading { since: 2, until: 17 }));
    assert_eq!(loop_update(&t, loop_call(3, LOOP_TAG, 0x55, 2, 0, 0.5, 2)), Ret::new(0, 32));
    // No tag: done, and the source is not read.
    let mut g = loop_call(3, TagIndex(NONE), 0x55, 0, 0, 0.0, 2);
    g.put_u32(STACK + 4 + 16, 0x0bad_0000);
    assert_eq!(loop_update(&t, g), Ret::new(1, 32));
    assert_eq!(lock(&t).ledger.refused_unreadable, 0);
}

#[test]
fn a_shared_loop_with_a_spatial_source_is_one_for_every_owner() {
    let t = loop_table();
    let def = TIME + 0x3600;
    let with_def = |mut g: FakeGuest| {
        g.put_u32(points::TAG_ENTRIES.base, TIME + 0x3500).put_u32(TIME + 0x3500 + 2 * 16 + 8, def).put_u8(def, 0x80);
        g.put_u32(SOURCE, 1 << 24);
        g
    };
    for owner in [1, 2] {
        assert_eq!(loop_update(&t, with_def(loop_call(0, LOOP_TAG, owner, 0, 0, 0.0, 0))), Ret::new(0, 32));
    }
    let t = lock(&t);
    assert_eq!(t.sounds.loops().len(), 1);
    assert!(t.sounds.loop_find(LOOP_TAG, LOOP_TAG.0 | 0x6000).is_some());
}

#[test]
fn a_stop_frees_a_loop_that_holds_no_sound_at_once() {
    let t = loop_table();
    assert_eq!(loop_update(&t, loop_call(0, LOOP_TAG, 0x55, 0, 0, 0.0, 0)), Ret::new(0, 32));
    let h = lock(&t).sounds.loop_find(LOOP_TAG, 0x55).unwrap();
    let primary = lock(&t).sounds.loops().get(h).unwrap().tracks[0].primary.unwrap();
    lock(&t).sounds.finish(primary, Reason::PlayedOut, 1, &mut NullOwners);
    assert_eq!(loop_update(&t, loop_call(1, LOOP_TAG, 0x55, 2, 0, 0.0, 0)), Ret::new(1, 32));
    assert!(lock(&t).sounds.loops().is_empty());
}

#[test]
fn the_ambience_blends_its_slots_toward_the_candidates_with_no_guest_call() {
    use crate::sim_sound::ambience::{FLOOR, FRAME_TIME, FULL, PAIR_SCALE, SLOTS};
    let t = table(Arc::new(Flat(10.0)));
    let mut g = guest(0);
    g.put_f32(FLOOR, 0.0).put_f32(PAIR_SCALE, 2.0).put_f32(FULL, 1.0).put_f32(GLOBALS + FRAME_TIME, 0.1);
    // Candidate 0: id 7 over 1 s; candidate 1 and both slots empty.
    g.put_f32(RECORD, 1.0).put_u32(RECORD + 8, 7).put_u32(RECORD + 0x1c + 8, NONE);
    g.put_u32(GLOBALS + SLOTS + 8, NONE).put_u32(GLOBALS + SLOTS + 0x1c + 8, NONE);
    let r = run_body(&t, Body::Ambience, g, Regs { eax: RECORD, ..regs() }, |c, _| panic!("no guest call: {:#x}", c.target));
    assert_eq!(r.result, Ok(Ret::new(0, 0)));
    assert_eq!(r.guest.u32(GLOBALS + SLOTS + 0x1c + 8), Ok(7), "the last free slot");
    assert_eq!(r.guest.f32(GLOBALS + SLOTS + 0x1c + 4), Ok(0.1));
}

const EFFECT: u32 = RECORD + 0x800;
/// A custom playback word: row 9 of the scenario's list (kind 2).
const CUSTOM: u32 = (2 << 30) | 9;

/// An effect in the map: its components (group, tag, gain, flags). Both
/// [`TAG`]'s bank row (now a `snd!`) and [`CUSTOM`]'s row name it.
fn with_effect(g: &mut FakeGuest, parts: &[(u32, TagIndex, f32, u32)]) {
    g.put_u32(TAGS + 16, crate::sim_sound::playback::SOUND_GROUP);
    g.put_u32(GESTALT, GESTALT_AT).put_u32(GESTALT_AT + 0x34, BANKS);
    g.put_u32(points::SCENARIO.base, SCENARIO_AT).put_u32(SCENARIO_AT + 0x324, 3);
    g.put_u32(TAGS + 3 * 16 + 8, LIST_DEF).put_u32(LIST_DEF + 4, LIST_ROWS);
    for row in [BANKS, LIST_ROWS + 9 * 0x38 + 4] {
        g.put_u32(row + 0x2c, 1).put_u32(row + 0x30, EFFECT);
    }
    g.put_u32(EFFECT + 8, parts.len() as u32).put_u32(EFFECT + 0xc, EFFECT + 0x10);
    for (i, (group, tag, gain, flags)) in parts.iter().enumerate() {
        let c = EFFECT + 0x10 + 16 * i as u32;
        g.put_u32(c, *group).put_u32(c + 4, tag.0).put_u32(c + 8, gain.to_bits()).put_u32(c + 0xc, *flags);
    }
}

#[test]
fn a_tag_start_admits_its_sound_and_starts_its_loop_on_its_own_handle() {
    let t = loop_table();
    let mut g = guest(4);
    with_effect(&mut g, &[(SOUND_GROUP, TAG, 0.5, 0), (LOOP_GROUP, LOOP_TAG, 0.25, 0)]);
    g.put_u32(STACK + 4, 3);
    with_gate(&mut g, true);
    let regs = Regs { ecx: CUSTOM, xmm0: u128::from(2.0f32.to_bits()), ..regs() };
    let r = run_body(&t, Body::StartLayerNew, g, regs, no_calls);
    let ret = r.result.unwrap();
    assert_eq!(ret.pop, 4);
    let start = Handle::from_raw(ret.eax);
    // The component's record: flags 0x95 with bit 5, the start's table.
    assert_eq!(component_record(&r), (0xb5, STARTED_TABLE));
    let t = lock(&t);
    let row = *t.sounds.starts().get(start).unwrap();
    assert_eq!((row.of, row.word, row.effect, row.flags()), (StartOf::Tag { scale: 2.0 }, 3, EFFECT, 0));
    let (_, sound) = t.sounds.instances().iter().find(|(_, i)| i.owner == Owner::Started(start)).expect("the owned sound");
    assert_eq!(sound.source.0[1], 2.0f32.to_bits(), "the start's scale");
    let looping = t.sounds.loop_find(LOOP_TAG, start.raw() | OWNER_BIT).expect("the start's loop");
    let source = t.sounds.loops().get(looping).unwrap().source;
    assert_eq!((source.0[1], source.0[2]), (2.0f32.to_bits(), 0.25f32.to_bits()), "the gain on word 2");
}

#[test]
fn a_tag_start_stops_its_loop_and_the_walk_frees_it() {
    let t = loop_table();
    let mut g = guest(4);
    // The second sound plays at the stop only (component flag bit 1).
    with_effect(&mut g, &[(SOUND_GROUP, TAG, 0.0, 0), (LOOP_GROUP, LOOP_TAG, 0.0, 0), (SOUND_GROUP, TAG, 0.0, 3)]);
    with_gate(&mut g, true);
    let r = run_body(&t, Body::StartLayerNew, g.clone(), Regs { ecx: CUSTOM, ..regs() }, no_calls);
    assert_eq!(component_record(&r), (0xb5, STARTED_TABLE));
    let start = Handle::from_raw(r.result.unwrap().eax);
    let looping = lock(&t).sounds.loop_find(LOOP_TAG, start.raw() | OWNER_BIT).unwrap();
    let r = run_body(&t, Body::StartLayerPlay, g.clone(), Regs { ecx: start.raw(), ..regs() }, no_calls);
    assert_eq!(r.result, Ok(Ret::new(0, 0)));
    assert_eq!(component_record(&r), (0x95, 0), "the stop's sound is not owned");
    {
        let t = lock(&t);
        assert_eq!(t.sounds.starts().get(start).unwrap().flags(), 0x19);
        assert!(t.sounds.loops().get(looping).is_none_or(|l| l.stopping));
    }
    // The walk frees the done row; the start's own sound then ends.
    let r = run_body(&t, Body::StartsUpdate, g, regs(), |c, _| panic!("a done row calls nothing: {:#x}", c.target));
    assert_eq!(r.result, Ok(Ret::new(0, 0)));
    let mut t = lock(&t);
    assert!(t.sounds.starts().is_empty());
    t.sounds.step(5, &mut NullOwners);
    assert!(t.sounds.instances().iter().all(|(_, i)| !matches!(i.owner, Owner::Started(_))));
    assert_eq!(t.sounds.counters().ended(Reason::OwnerGone), 1);
}

#[test]
fn a_sound_start_binds_its_record_and_its_stop_runs_the_starts_stop() {
    let t = loop_table();
    let mut g = with_tag(guest(4));
    record(&mut g);
    with_effect(&mut g, &[(SOUND_GROUP, TAG, 0.0, 0)]);
    g.put_u16(RECORD + 4, 0x0102);
    with_gate(&mut g, true);
    let r = run_body(&t, Body::StartRecord, g.clone(), Regs { eax: RECORD, ..regs() }, no_calls);
    let sound = Handle::from_raw(r.result.unwrap().eax);
    // The record now names the start's table (`0x21dc60`); then the component's.
    assert_eq!((r.guest.u32(RECORD), r.guest.u32(RECORD + 0x58)), (Ok((1 << 10) | 0x20), Ok(SOUND_TABLE)));
    assert_eq!(component_record(&r), (0xb5, STARTED_TABLE));
    assert_eq!(r.guest.u32(RECORD + request::OWN_TABLE_AT), Ok(0));
    let start = {
        let t = lock(&t);
        let (start, row) = t.sounds.starts().iter().next().unwrap();
        assert_eq!((row.of, row.word, row.flags()), (StartOf::Sound(sound), 0x0102, 0x08), "no owner of its own: released");
        assert_eq!(t.sounds.get(sound).unwrap().owner, Owner::Start { start, own: Own::None });
        assert_eq!(t.sounds.instances().len(), 2);
        start
    };
    // Its sound's stop gives the sound back and stops the start, cut.
    let r = run_body(&t, Body::Stop, g, Regs { ebx: sound.raw(), ..regs() }, no_calls);
    assert_eq!(r.result, Ok(Ret::new(0, 0)));
    let t = lock(&t);
    assert_eq!(t.sounds.get(sound).unwrap().owner, Owner::None);
    assert_eq!(t.sounds.starts().get(start).unwrap().flags(), 0x1b);
    assert_eq!(component_record(&r), (0, 0), "no component plays at the stop");
}

#[test]
fn a_refused_sound_start_frees_its_row() {
    let t = loop_table();
    let mut g = with_tag(guest(4));
    record(&mut g);
    with_effect(&mut g, &[(SOUND_GROUP, TAG, 0.0, 0)]);
    with_gate(&mut g, false);
    let r = run_body(&t, Body::StartRecord, g, Regs { eax: RECORD, ..regs() }, no_calls);
    assert_eq!(r.result.unwrap().eax, NONE);
    let t = lock(&t);
    assert!(t.sounds.starts().is_empty());
    assert!(t.sounds.instances().is_empty());
}

#[test]
fn the_walk_releases_a_sound_start_whose_unit_is_gone() {
    let t = loop_table();
    let mut g = with_tag(guest(4));
    // A unit's line (flag bit 5, finish `0x18c8c0`); the objects array is empty.
    let unit_table = RECORD + 0x700;
    g.put_u32(RECORD, (1 << 10) | (1 << 5)).put_u32(RECORD + 0x58, unit_table).put_u32(RECORD + 0x5c, 0xe001_0007);
    g.put_u32(unit_table + 0x10, request::UNIT_FINISH);
    with_effect(&mut g, &[(SOUND_GROUP, TAG, 0.0, 1)]);
    with_gate(&mut g, true);
    run_body(&t, Body::StartRecord, g.clone(), Regs { eax: RECORD, ..regs() }, no_calls);
    let start = lock(&t).sounds.starts().handles()[0];
    assert!(!lock(&t).sounds.starts().get(start).unwrap().released);
    let r = run_body(&t, Body::StartsUpdate, g, regs(), no_calls);
    assert!(lock(&t).sounds.starts().get(start).unwrap().released);
    assert_eq!(component_record(&r), (0, 0), "an update plays no sound");
}
