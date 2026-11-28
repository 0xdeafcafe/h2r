//! Zones, the roster's events and loadouts on the host
//! (`super::super::world::zones`, `super::super::world::loadout`), over
//! invented guest memory: two territory flags, two players with units
//! whose positions the test moves, and fake engine callees.

use guest_layout::view::Datums;
use guest_layout::view::objects::{Body, Header};

use super::*;
use crate::rules::world::loadout;

const SCNR: u32 = 0x1006_0000;
const FLAGS_AT: u32 = SCNR + 0x400;
const OBJECTS: u32 = 0x1007_0000;
const BODIES: u32 = 0x1008_0000;
const SCRATCH: u32 = 0x1009_0000;
const TERRITORY: u16 = engine::objectives::kind::TERRITORY;
const BR: &str = "objects/weapons/rifle/battle_rifle/battle_rifle";
const MAGNUM: &str = "objects/weapons/pistol/magnum/magnum";
/// The body depleted bit (`Body::damage_flags`).
const DEPLETED: u16 = 1 << 2;

/// Player `i`'s unit body.
fn body_of(i: usize) -> Body {
    Body::at(BODIES + i as u32 * 0x400)
}

/// [`guest`] whose record names `ir`, with a scenario of `flags`
/// (position, type, team) and players 0 and 1 (teams 0 and 1) in living
/// units at `at`. Returns the guest, the runtime and the units' datums.
fn world(ir: &Ir, flags: &[([f32; 3], u16, u16)], at: [[f32; 3]; 2]) -> (FakeGuest, Arc<Shared>, [u32; 2]) {
    let store = Local::default();
    let blob = ir.encode();
    let s = shared(&store, Box::new(move |_: [u8; 8]| Ok::<_, String>(blob.clone())));
    let (mut m, _) = guest(block(prefix(&ir.id()), 0), SLAYER);
    m.put_u32(points::SCENARIO.base, SCNR);
    m.map(SCNR, 0x1000);
    m.put_u32(SCNR + engine::objectives::NETGAME_FLAGS, flags.len() as u32)
        .put_u32(SCNR + engine::objectives::NETGAME_FLAGS + 4, FLAGS_AT);
    for (i, (p, kind, team)) in flags.iter().enumerate() {
        let f = FLAGS_AT + i as u32 * engine::objectives::NETGAME_FLAG_STRIDE;
        m.put_f32(f, p[0])
            .put_f32(f + 4, p[1])
            .put_f32(f + 8, p[2])
            .put_u16(f + engine::objectives::FLAG_TYPE, *kind)
            .put_u16(f + engine::objectives::FLAG_TEAM, *team);
    }
    m.put_u32(points::OBJECTS.base, OBJECTS);
    m.map(SCRATCH, loadout::SCRATCH_SIZE);
    let objects = fake::datums::<Header>(&mut m, OBJECTS, 8);
    let players = Datums::<Player>::at(PLAYERS);
    let mut units = [0; 2];
    for (i, p) in at.iter().enumerate() {
        let (unit, header) = fake::push(&mut m, &objects);
        let body = body_of(i);
        m.map(BODIES + i as u32 * 0x400, 0x400);
        header.set_body(&mut m, BODIES + i as u32 * 0x400).unwrap();
        body.set_position(&mut m, *p).unwrap();
        let player = players.get_absolute(&m, i as i32).unwrap().unwrap();
        player.set_unit(&mut m, Some(unit)).unwrap();
        units[i] = unit.raw();
    }
    (m, s, units)
}

/// One game tick `n` after the start: the boundary, then the tick's end.
fn tick(s: &Arc<Shared>, m: &mut FakeGuest, n: u32) -> Option<Work> {
    m.put_u32(GAME_TIME + 8, TICK + n);
    let work = s.lock().pre_tick(m);
    s.lock().post_tick(m, &[]);
    work
}

fn global(s: &Arc<Shared>, name: &str) -> i32 {
    s.lock().running().unwrap().vm.global(name).unwrap()
}

#[test]
fn crouching_reads_the_living_bipeds_crouch_request() {
    use crate::superbounce_chain::{Medals, read_players};
    use live_telemetry::layout::{UNIT_FLAG_CROUCH, UNIT_FLAGS};

    let ir = ::rules::compile(r#"
rules "Crouch" 1
type slayer
requires hud.superbounce
global var crouched: int = 0
on tick { crouched = count(players p where p.crouching) }
"#).unwrap();
    let (mut m, s, _) = world(&ir, &[], [[0.0; 3], [5.0, 0.0, 0.0]]);
    let line = s.lock().start(&m, false);
    assert!(s.lock().state().running, "{line}");

    let mut medals = Medals::new();
    let mut step = |m: &mut FakeGuest, n: u32| {
        medals.step(TICK + n, &read_players(m), &[]);
        s.lock().bodies(medals.bodies());
        tick(&s, m, n);
        global(&s, "crouched")
    };
    assert_eq!(step(&mut m, 1), 0);
    m.put_u32(body_of(0).addr + UNIT_FLAGS, UNIT_FLAG_CROUCH);
    assert_eq!(step(&mut m, 2), 1);
    m.put_u32(body_of(0).addr + UNIT_FLAGS, 0);
    assert_eq!(step(&mut m, 3), 0);
    m.put_u32(body_of(0).addr + UNIT_FLAGS, UNIT_FLAG_CROUCH);
    body_of(0).set_damage_flags(&mut m, DEPLETED).unwrap();
    assert_eq!(step(&mut m, 4), 0);

    let objects = Datums::<Header>::at(OBJECTS);
    let vehicle = objects.get_absolute(&m, 1).unwrap().unwrap();
    vehicle.set_kind(&mut m, guest_layout::view::objects::VEHICLE).unwrap();
    m.put_u32(body_of(1).addr + UNIT_FLAGS, UNIT_FLAG_CROUCH);
    assert_eq!(step(&mut m, 5), 0);
}

#[test]
fn a_biped_read_names_its_missing_superbounce_requirement_at_start() {
    for name in ["airborne", "crouching", "primed"] {
        let src = format!(
            "rules \"Biped\" 1\ntype slayer\non tick {{ for each players p {{ if p.{name} {{ score(p, 1) }} }} }}\n"
        );
        let ir = ::rules::compile(&src).unwrap();
        let (m, s, _) = world(&ir, &[], [[0.0; 3]; 2]);
        let line = s.lock().start(&m, false);
        assert!(line.contains(&format!("the script reads {name},")), "{line}");
        assert!(line.contains("requires hud.superbounce"), "{line}");
        assert!(s.lock().running().is_none());
    }
}

const ZONES: &str = r#"
rules "Zones" 1
type slayer
label site: zones at least 1 = "territory"
global var entered: int = 0
global var left: int = 0
global var last: player? = none
global var held: time = 0t
global var inside_now: int = 0
on enter(site z, o) {
    entered = entered + 1
    last = player_of(o)
}
on leave(site z, o) { left = left + 1 }
on tick {
    held = 0t
    for each site s { held = held + ticks_held(s, team 0) }
    inside_now = count(players p where count(site s where inside(p, s)) > 0)
}
"#;

#[test]
fn a_unit_entering_a_flags_zone_is_told_once_and_held_by_its_team() {
    let ir = ::rules::compile(ZONES).unwrap();
    assert_eq!(host_refusal(&ir), None);
    let flags = [([0.0, 0.0, 0.0], TERRITORY, 0), ([20.0, 0.0, 0.0], TERRITORY, 0)];
    // Player 0 stands on the first territory; player 1 on neither.
    let (mut m, s, _) = world(&ir, &flags, [[0.2, 0.0, 0.5], [10.0, 0.0, 0.0]]);
    let line = s.lock().start(&m, false);
    assert!(s.lock().state().running, "{line}");
    let sites = s.lock().running().unwrap().table.labelled("territory");
    let (a, b) = (sites[0].object, sites[1].object);
    assert!(s.lock().running().unwrap().zones.contains(a, [0.0; 3]));

    tick(&s, &mut m, 1);
    assert_eq!(global(&s, "entered"), 1);
    assert_eq!(global(&s, "last"), 0, "player_of reads the roster's unit");
    assert_eq!(global(&s, "inside_now"), 1);
    // Still there: nothing new is told, and team 0 holds it each tick.
    tick(&s, &mut m, 2);
    tick(&s, &mut m, 3);
    assert_eq!(global(&s, "entered"), 1);
    let vm_held = |s: &Arc<Shared>, zone: u32, team: u8| s.lock().running().unwrap().vm.ticks_held(zone, team);
    assert_eq!(vm_held(&s, a, 0), 2, "counted from the tick after it was first named");
    assert_eq!(vm_held(&s, b, 0), 0);
    assert_eq!(global(&s, "held"), 2);

    // Player 1 joins it: contested, so no team holds it.
    body_of(1).set_position(&mut m, [0.0, 0.3, 0.0]).unwrap();
    tick(&s, &mut m, 4);
    assert_eq!(global(&s, "entered"), 2);
    assert_eq!(global(&s, "last"), 1);
    assert_eq!(global(&s, "inside_now"), 2);
    assert_eq!(vm_held(&s, a, 0), 2);
    assert_eq!(vm_held(&s, a, 1), 0);

    // Player 0 walks off: a leave, and team 1 holds it alone from this
    // tick.
    body_of(0).set_position(&mut m, [10.0, 0.0, 0.0]).unwrap();
    tick(&s, &mut m, 5);
    assert_eq!(global(&s, "left"), 1);
    assert_eq!(vm_held(&s, a, 1), 1);
    tick(&s, &mut m, 6);
    assert_eq!(vm_held(&s, a, 1), 2);
    assert_eq!(vm_held(&s, a, 0), 2);

    // Player 1 dies on it: its unit leaves the zone.
    body_of(1).set_damage_flags(&mut m, DEPLETED).unwrap();
    tick(&s, &mut m, 7);
    assert_eq!(global(&s, "left"), 2);
    assert_eq!(global(&s, "inside_now"), 0);
}

/// `rules.zones` reports each flag's zone with where its shape came from.
#[test]
fn the_zones_report_names_each_flags_shape() {
    let ir = ::rules::compile(ZONES).unwrap();
    let flags = [([0.0, 0.0, 0.0], TERRITORY, 0), ([20.0, 0.0, 3.0], TERRITORY, 0)];
    let (m, s, _) = world(&ir, &flags, [[0.2, 0.0, 0.5], [10.0, 0.0, 0.0]]);
    assert!(s.lock().zones().zones.is_empty(), "no script runs yet");
    let line = s.lock().start(&m, false);
    assert!(s.lock().state().running, "{line}");
    let report = s.lock().zones();
    assert_eq!(report.zones.len(), 2);
    let z = &report.zones[1];
    assert_eq!((z.label.as_str(), z.shape.as_str(), z.reports), ("territory", "default", true));
    assert_eq!(z.at, [20.0, 0.0, 3.0]);
    assert_eq!(z.area, serde_json::json!({"radius": 1.0, "below": 0.5, "above": 1.5}));
    assert!(report.zones[0].id < z.id);
}

const KIT: &str = r#"
rules "Kit" 1
type slayer
requires player.weapons
global var spawns: int = 0
global var deaths: int = 0
on spawn(p) {
    spawns = spawns + 1
    set_loadout(p, "objects/weapons/rifle/battle_rifle/battle_rifle", "objects/weapons/pistol/magnum/magnum")
}
on death(p) { deaths = deaths + 1 }
"#;

/// The fake game's answers: the authority yes, a weapon value's tag
/// `0xe100_0000 | value`, a made weapon `0xe200_0000 | tag's value`, and
/// `unit_add_weapon` `added`.
fn answer(c: &GuestCall, added: bool) -> Regs {
    use agent::sim::calls::addr;
    let eax = match c.target {
        t if t == points::GAME_ENGINE_AUTHORITATIVE.base => 1,
        t if t == points::STARTING_WEAPON_PICK.base => 0xe100_0000 | c.args[1],
        addr::OBJECT_NEW => 0xe200_0000 | 7,
        addr::UNIT_ADD_WEAPON => u32::from(added),
        _ => 0,
    };
    Regs {
        eax,
        ..Default::default()
    }
}

fn run(s: &Arc<Shared>, m: &FakeGuest, work: Work, added: bool) -> Vec<GuestCall> {
    let shared = Arc::clone(s);
    let entry = move |cx: Cx| -> EntryFuture { Box::pin(run_work(cx, Arc::clone(&shared), work.clone(), SCRATCH)) };
    let run = testing::run(&entry, m.clone(), Regs::default(), |c, _| answer(c, added));
    assert_eq!(run.result, Ok(Ret::new(0, 0)));
    run.calls
}

/// The calls one weapon of a loadout makes.
fn give(value: u32, unit: u32, mode: u32) -> Vec<GuestCall> {
    use agent::sim::calls::addr;
    let tag = 0xe100_0000 | value;
    vec![
        GuestCall::new(points::STARTING_WEAPON_PICK.base).args([u32::MAX, value, u32::MAX]),
        GuestCall::new(addr::PLACEMENT_NEW).esi(SCRATCH).args([tag, unit, 0]),
        GuestCall::new(addr::OBJECT_NEW).args([SCRATCH]),
        GuestCall::new(addr::UNIT_ADD_WEAPON).args([unit, 0xe200_0007, mode]),
    ]
}

#[test]
fn a_spawn_gets_its_loadout_at_the_next_boundary() {
    use agent::sim::calls::addr;
    loadout::set_scratch(SCRATCH);
    let ir = ::rules::compile(KIT).unwrap();
    assert_eq!(host_refusal(&ir), None);
    let (mut m, s, units) = world(&ir, &[], [[0.0; 3], [5.0, 0.0, 0.0]]);
    s.lock().start(&m, false);
    assert!(s.lock().state().running);

    // Both are alive at the first tick's end: two spawns.
    tick(&s, &mut m, 1);
    assert_eq!(global(&s, "spawns"), 2);
    let kit = |player: u8| Call::SetLoadout {
        player,
        primary: Some(BR.into()),
        secondary: Some(MAGNUM.into()),
    };
    assert_eq!(s.lock().running().unwrap().pending(), [kit(0), kit(1)]);
    // Nothing more while they live.
    let work = tick(&s, &mut m, 2).expect("the calls");
    assert_eq!(global(&s, "spawns"), 2);
    assert_eq!(work.calls, [kit(0), kit(1)]);

    // The primary empties the slots (mode 2), the secondary joins it.
    let calls = run(&s, &m, work, true);
    let mut want = vec![GuestCall::new(points::GAME_ENGINE_AUTHORITATIVE.base)];
    for unit in units {
        want.extend(give(3, unit, addr::MODE_REPLACE));
        want.extend(give(4, unit, addr::MODE_ADD));
    }
    want.push(GuestCall::new(points::GAME_TICK.base));
    assert_eq!(calls, want);
    assert!(s.lock().state().errors.is_empty(), "{:?}", s.lock().state().errors);

    // Player 0 dies, then lives again: a death, then a spawn and its loadout.
    body_of(0).set_damage_flags(&mut m, DEPLETED).unwrap();
    tick(&s, &mut m, 3);
    assert_eq!(global(&s, "deaths"), 1);
    body_of(0).set_damage_flags(&mut m, 0).unwrap();
    tick(&s, &mut m, 4);
    assert_eq!(global(&s, "spawns"), 3);
    assert_eq!(s.lock().running().unwrap().pending(), [kit(0)]);

    // A unit that refuses the weapon: it is deleted, and the call is named.
    let work = tick(&s, &mut m, 5).expect("the call");
    let calls = run(&s, &m, work, false);
    assert_eq!(
        calls.iter().map(|c| c.target).collect::<Vec<_>>(),
        [
            points::STARTING_WEAPON_PICK.base,
            addr::PLACEMENT_NEW,
            addr::OBJECT_NEW,
            addr::UNIT_ADD_WEAPON,
            addr::OBJECT_DELETE,
            points::GAME_TICK.base,
        ]
    );
    let errors = s.lock().state().errors;
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].call, "set_loadout");
    assert!(errors[0].error.contains("refused"), "{errors:?}");
}

#[test]
fn a_script_naming_a_weapon_off_the_list_is_refused_by_its_path() {
    let src = KIT.replace(MAGNUM, "objects/weapons/multiplayer/flag/flag");
    let ir = ::rules::compile(&src).unwrap();
    let why = host_refusal(&ir).unwrap();
    assert!(why.contains("objects/weapons/multiplayer/flag/flag"), "{why}");
    assert!(why.contains("set_loadout"), "{why}");
}

/// For the remaining vehicle entries, a host refuses a script
/// that calls one, by name and by why (docs/game/rules-world-api.md,
/// "Vehicles and drop ships"), before its game starts.
#[test]
fn a_script_that_flies_a_ship_is_refused_by_the_entry_it_calls() {
    for (name, call, status) in [
        ("fly", "fly(ship, run_in, 10t)", "not built"),
        ("eject", "if let p = first(players q) { eject(p) }", "traced but not a named point"),
    ] {
        let src = format!(
            "rules \"Drop\" 1\ntype slayer\nrequires vehicles.drive\nlabel ship: object\nlabel run_in: path\non tick {{ {call} }}\n"
        );
        let ir = ::rules::compile(&src).unwrap_or_else(|e| panic!("{call}: {e:?}"));
        let why = host_refusal(&ir).unwrap();
        assert!(why.contains(name) && why.contains(status), "{call}: {why}");
    }
}

#[test]
fn a_labelled_ship_queues_hover_and_unload_for_its_salted_datum() {
    use crate::rules::world::vehicles::Action::{Hover, Unload};
    let ir = ::rules::compile(r#"
rules "Drop" 1
type slayer
requires vehicles.drive
label ship: object
on init {
    hover(ship, true)
    hover(ship, false)
    unload(ship)
}
"#).unwrap();
    assert_eq!(host_refusal(&ir), None);
    let (mut m, s, units) = world(&ir, &[], [[0.0; 3]; 2]);
    let v = units[0];
    let array = Datums::<Header>::at(OBJECTS);
    let header = array.get_absolute(&m, 0).unwrap().unwrap();
    header.set_kind(&mut m, guest_layout::view::objects::VEHICLE).unwrap();
    m.put_u16(body_of(0).addr + guest_layout::Object::PLACEMENT_INDEX.offset, 3);
    let table = Table::from_things(vec![census::Thing {
        text: "ship".into(),
        label: Some(0),
        number: None,
        class: ::rules::labels::Class::Object,
        designator: None,
        block: forge_doc::BlockKind::Vehicles,
        index: 3,
        area: None,
    }]);
    let mut calls = Vec::new();
    let mut game = GuestGame::new(&m, &mut calls).with_table(&table);
    let mut vm = Vm::new(ir, 0, 1).unwrap();
    vm.begin_tick(TICK);
    assert_eq!(vm.fire(&mut game, Input::Init), Fired::Ran);
    vm.flush_queue(&mut game);
    assert_eq!(calls, [
        Call::Vehicle(Hover(true), Some(v)),
        Call::Vehicle(Hover(false), Some(v)),
        Call::Vehicle(Unload, Some(v)),
    ]);
    let work = Work { tick: TICK, ask_authority: true, calls, sight: Vec::new() };
    assert_eq!(run(&s, &m, work.clone(), false), [
        GuestCall::new(points::GAME_ENGINE_AUTHORITATIVE.base),
        GuestCall::new(points::VEHICLE_HOVER.base).eax(v).ecx(1),
        GuestCall::new(points::VEHICLE_HOVER.base).eax(v).ecx(0),
        GuestCall::new(points::VEHICLE_UNLOAD.base).edx(v).ecx(u32::MAX),
        GuestCall::new(points::GAME_TICK.base),
    ]);
    // Even a replacement vehicle at the same placement and slot cannot
    // inherit the queued requests.
    header.set_salt(&mut m, ((v >> 16) as u16) ^ 1).unwrap();
    assert_eq!(run(&s, &m, work, false), [
        GuestCall::new(points::GAME_ENGINE_AUTHORITATIVE.base),
        GuestCall::new(points::GAME_TICK.base),
    ]);
}

#[test]
fn sees_answers_from_the_next_boundary_and_refreshes_each_tick() {
    let ir = ::rules::compile(r#"
rules "Sight" 1
type slayer
label eye: object?
label target: object?
global var seen: bool = false
on tick {
    if let a = eye {
        if let b = target { seen = sees(a, b) }
    }
}
"#).unwrap();
    assert_eq!(host_refusal(&ir), None);
    let positions = [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]];
    let (mut m, s, units) = world(&ir, &[], positions);
    let array = Datums::<Header>::at(OBJECTS);
    for i in 0..2 {
        let header = array.get_absolute(&m, i).unwrap().unwrap();
        header.set_kind(&mut m, guest_layout::view::objects::VEHICLE).unwrap();
        let body = body_of(i as usize);
        body.set_parent(&mut m, None).unwrap();
        m.put_u16(body.addr + guest_layout::Object::PLACEMENT_INDEX.offset, i as u16);
    }
    s.lock().start(&m, false);
    {
        // The two stand-in object placements have the same kind and
        // different indices, as they would in the scenario census.
        let mut rt = s.lock();
        let Status::Running(r) = &mut rt.status else {
            panic!("the host refused the sees script");
        };
        r.table = Table::from_things(
            ["eye", "target"].into_iter().enumerate().map(|(i, name)| census::Thing {
                text: name.into(),
                label: Some(i as u16),
                number: None,
                class: ::rules::labels::Class::Object,
                designator: None,
                block: forge_doc::BlockKind::Vehicles,
                index: i as u32,
                area: None,
            }).collect(),
        );
    }
    let work = s.lock().pre_tick(&m).unwrap();
    assert!(work.sight.is_empty(), "no handler has asked yet");
    run(&s, &m, work, false);
    m.put_u32(GAME_TIME + 8, TICK + 1);
    s.lock().post_tick(&m, &[]);
    assert_eq!(global(&s, "seen"), 0, "false on the first request");

    for (n, clear) in [(2, true), (3, false)] {
        let work = s.lock().pre_tick(&m).expect("sight alone schedules boundary work");
        assert!(!work.ask_authority);
        assert!(work.calls.is_empty());
        assert_eq!(work.sight.len(), 1);
        let shared = Arc::clone(&s);
        let entry = move |cx: Cx| -> EntryFuture {
            Box::pin(run_work(cx, Arc::clone(&shared), work.clone(), SCRATCH))
        };
        let run = testing::run(&entry, m.clone(), Regs::default(), |call, guest| {
            if call.target == points::AI_LINE_OF_SIGHT.base {
                assert_eq!(guest.vec3(SCRATCH).unwrap(), positions[0]);
                assert_eq!(guest.vec3(SCRATCH + 12).unwrap(), positions[1]);
            }
            Regs { eax: u32::from(call.target == points::AI_LINE_OF_SIGHT.base && clear), ..Default::default() }
        });
        assert_eq!(run.result, Ok(Ret::new(0, 0)));
        assert_eq!(run.calls, [
            GuestCall::new(points::AI_LINE_OF_SIGHT.base)
                .esi(SCRATCH).edi(SCRATCH + 12).edx(units[0]).ecx(units[1]),
            GuestCall::new(points::GAME_TICK.base),
        ]);
        m.put_u32(GAME_TIME + 8, TICK + n);
        s.lock().post_tick(&m, &[]);
        assert_eq!(global(&s, "seen"), i32::from(clear));
    }
}

/// Every listed script a host scores runs here: none is listed only to be
/// refused when the game starts.
#[test]
fn every_listed_scripted_mode_is_hostable() {
    let mut scripted = Vec::new();
    for b in ::rules::builtin::LISTED {
        let ir = ::rules::compile(b.source).unwrap();
        if ir.header.uses.is_some() {
            continue;
        }
        assert_eq!(host_refusal(&ir), None, "{}", b.name);
        scripted.push(b.name);
    }
    for name in ["Siege", "Gun Game", "Race", "VIP"] {
        assert!(scripted.contains(&name), "{name}: {scripted:?}");
    }
}

const LINES: &str = r#"
rules "Lines" 1
type slayer
teams 2
requires player.spawn
label site: zones = "territory"
on round_start { for each site s { spawn_near(team 1, s, 2) } }
"#;

/// `spawn_near` keeps team 1's spawns to the two starting locations
/// nearest the territory, once this machine makes the game's rules; team
/// 0 has no rule, so the engine picks for it.
#[test]
fn a_teams_spawn_rule_keeps_its_spawns_to_the_nearest_locations() {
    use std::collections::BTreeSet;
    const STARTS_AT: u32 = SCNR + 0x800;
    let ir = ::rules::compile(LINES).unwrap();
    assert_eq!(host_refusal(&ir), None);
    let (mut m, s, _) = world(&ir, &[([9.0, 0.0, 0.0], TERRITORY, 8)], [[0.0; 3], [5.0, 0.0, 0.0]]);
    let starts = map::scenario_layout::block(forge_doc::BlockKind::StartingLocations);
    m.put_u32(SCNR + starts.offset, 4)
        .put_u32(SCNR + starts.offset + 4, STARTS_AT);
    for (i, x) in [0.0f32, 4.0, 8.0, 10.0].into_iter().enumerate() {
        m.put_f32(STARTS_AT + i as u32 * starts.stride, x);
    }
    s.lock().start(&m, false);
    assert!(s.lock().state().running);
    assert_eq!(s.lock().spawn_filter(&m, 1), None, "no authority yet");
    let work = tick(&s, &mut m, 1).expect("the authority");
    run(&s, &m, work, true);
    let f = s.lock().spawn_filter(&m, 1).expect("team 1's rule");
    assert_eq!(f.base, STARTS_AT);
    assert_eq!(f.live, BTreeSet::from([2, 3]));
    assert!(f.passes(STARTS_AT + 2 * starts.stride));
    assert!(!f.passes(STARTS_AT));
    assert_eq!(s.lock().spawn_filter(&m, 0), None, "team 0 has no rule");
}

const SIDES: &str = r#"
rules "Sides" 1
type slayer
teams 2
requires player.disguise
on round_start { set_body(team 1, "objects/characters/elite/elite_mp") }
"#;

/// `set_body` gives team 1's spawns the Elite's model byte, once this
/// machine makes the game's rules; the spawn's hooks write it into the
/// player's configuration for the spawn and put the player's own back.
#[test]
fn a_teams_body_is_the_model_byte_for_its_players_spawns() {
    use crate::rules::world::body;
    use ::rules::bodies::Body;
    let ir = ::rules::compile(SIDES).unwrap();
    assert_eq!(host_refusal(&ir), None);
    let (mut m, s, _) = world(&ir, &[], [[0.0; 3], [5.0, 0.0, 0.0]]);
    s.lock().start(&m, false);
    assert!(s.lock().state().running);
    assert_eq!(s.lock().spawn_body(&m, 1), None, "no authority yet");
    let work = tick(&s, &mut m, 1).expect("the authority");
    run(&s, &m, work, true);
    assert_eq!(s.lock().spawn_body(&m, 1), Some((1, Body::Elite)));
    assert_eq!(s.lock().spawn_body(&m, 0), None, "team 0 keeps its own");

    let at = crate::firefight::guest::player_record(&m, 1).unwrap() + body::PLAYER_MODEL;
    assert_eq!(m.u8(at).unwrap(), 0, "player 1 chose the Spartan");
    let swap = body::put(&mut m, 1, Body::Elite).unwrap().expect("a change");
    assert_eq!(m.u8(at).unwrap(), 1);
    body::restore(&mut m, swap).unwrap();
    assert_eq!(m.u8(at).unwrap(), 0, "the player's own again");
    // A player already in the body is left alone.
    assert_eq!(body::put(&mut m, 1, Body::Spartan).unwrap(), None);
}

const WALLS: &str = r#"
rules "Walls" 1
type slayer
teams 2
label site: zones at least 1 = "territory"
global var phase: int = 0
on tick {
    phase = phase + 1
    if phase == 1 { for each site s { show_area(s, team 1) } }
    if phase == 2 {
        if let z = first(site where true) { show_area(z, none) }
    }
    if phase == 3 { for each site s { hide_area(s) } }
}
"#;

/// A script's `show_area` puts the zone's own area in the walls the
/// runtime hands the window, in its team's colour, one at a time as the
/// script hides and shows; a zone with no area of its own has the default
/// cylinder round its flag.
#[test]
fn a_shown_area_is_a_wall_of_the_zones_shape_until_it_is_hidden() {
    let ir = ::rules::compile(WALLS).unwrap();
    assert_eq!(host_refusal(&ir), None);
    let flags = [([0.0, 0.0, 0.0], TERRITORY, 0), ([20.0, 0.0, 0.0], TERRITORY, 0)];
    let (mut m, s, _) = world(&ir, &flags, [[0.2, 0.0, 0.5], [10.0, 0.0, 0.0]]);
    s.lock().start(&m, false);
    assert!(s.lock().state().running);
    assert!(s.lock().walls().is_empty(), "a script that has not called it shows none");

    tick(&s, &mut m, 1);
    let walls = s.lock().walls();
    assert_eq!(walls.len(), 2);
    assert!(walls.iter().all(|w| w.team == Some(1)));
    let mut at: Vec<f32> = walls.iter().map(|w| w.at[0]).collect();
    at.sort_by(f32::total_cmp);
    assert_eq!(at, [0.0, 20.0]);
    assert_eq!(walls[0].area, game_markers::Area::default());

    tick(&s, &mut m, 2);
    let walls = s.lock().walls();
    assert_eq!(walls.iter().filter(|w| w.team.is_none()).count(), 1, "one recoloured to neutral");
    assert_eq!(walls.len(), 2);

    tick(&s, &mut m, 3);
    assert!(s.lock().walls().is_empty(), "hidden");
}

/// A client that is not the authority draws the walls the host's script
/// shows: the cue carries the shown areas, and the client's own copy of the
/// map gives each its shape, so the two machines have the same walls.
#[test]
fn a_client_reads_the_hosts_shown_areas_into_the_same_walls() {
    use crate::rules::world::walls;
    use ::rules::show::{Cue, WALLS as WALLS_KIND};
    let ir = ::rules::compile(WALLS).unwrap();
    let flags = [([0.0, 0.0, 0.0], TERRITORY, 0), ([20.0, 0.0, 0.0], TERRITORY, 0)];
    let (mut m, s, _) = world(&ir, &flags, [[0.2, 0.0, 0.5], [10.0, 0.0, 0.0]]);
    s.lock().start(&m, false);
    tick(&s, &mut m, 1);
    tick(&s, &mut m, 2);
    let host = s.lock().walls();
    assert_eq!(host.len(), 2);
    let shown = s.lock().running().unwrap().vm.shown_areas().to_vec();

    // Over the wire and back, then read on a machine with no VM.
    let bytes = Cue::Walls(shown.clone()).encode().unwrap();
    let Cue::Walls(received) = Cue::decode(WALLS_KIND, &bytes, &[]).unwrap() else {
        panic!("a walls cue")
    };
    assert_eq!(received, shown);
    assert_eq!(walls::client(&m, &received), host);
    // A zone the client's map has no flag for has no wall.
    assert!(walls::client(&m, &[(::rules::show::ZONE_BASE | 9, None)]).is_empty());
    // The whole list, none shown included, takes the walls off.
    assert!(walls::client(&m, &[]).is_empty());
}

#[test]
fn the_authority_sends_the_walls_when_they_change_and_repeats_them() {
    let shown = vec![(::rules::show::ZONE_BASE | 1, Some(0))];
    let none: Vec<(u32, Option<u8>)> = Vec::new();
    let sent = |list: &[(u32, Option<u8>)], at: u32| Some((list.to_vec(), at));
    // Nothing shown and nothing sent is nothing to say.
    assert!(!walls_due(None, &none, 5));
    // A first list, and a changed one, go at once.
    assert!(walls_due(None, &shown, 5));
    let other = vec![(::rules::show::ZONE_BASE | 1, None)];
    assert!(walls_due(sent(&shown, 5).as_ref(), &other, 6));
    // The same list waits out the repeat, so a late joiner has it.
    let repeat = ::rules::show::WALLS_REPEAT;
    assert!(!walls_due(sent(&shown, 5).as_ref(), &shown, 5 + repeat - 1));
    assert!(walls_due(sent(&shown, 5).as_ref(), &shown, 5 + repeat));
    // An emptied list goes once, then stays quiet.
    assert!(walls_due(sent(&shown, 5).as_ref(), &none, 6));
    assert!(!walls_due(sent(&none, 6).as_ref(), &none, 6 + repeat * 3));
}

const DAMAGE: &str = r#"
rules "Damage" 1
type slayer
global var hits: int = 0
global var last_amount: fixed = 0.0
global var last_by: player? = none
global var at_tick: int = -1
on damage(o, by, amount) {
    hits = hits + 1
    last_amount = amount
    last_by = by
}
on tick { at_tick = hits }
"#;

fn damage_event(victim: u32, player: guest_layout::Datum, body: f32) -> sim_state::SimEvent {
    sim_state::SimEvent::Damage(sim_state::Damage {
        tick: Some(1),
        victim: guest_layout::Datum(victim),
        victim_kind: 0,
        victim_player: guest_layout::Datum::NONE,
        owner_player: player,
        owner_object: guest_layout::Datum::NONE,
        owner_team: 0,
        code: 0,
        effect: guest_layout::Datum::NONE,
        flags: 0,
        body,
        shield: 0.0,
        direction: [0.0; 3],
        origin: [0.0; 3],
        position: None,
    })
}

/// A tick's damage events reach `on damage` before `on tick`, the first
/// 16 in event order, and only that tick's.
#[test]
fn the_ticks_damage_events_fire_on_damage_before_on_tick() {
    let ir = ::rules::compile(DAMAGE).unwrap();
    assert_eq!(host_refusal(&ir), None);
    let (mut m, s, _) = world(&ir, &[], [[0.0; 3], [10.0, 0.0, 0.0]]);
    let line = s.lock().start(&m, false);
    assert!(s.lock().state().running, "{line}");

    // Twenty events: only the first sixteen fire; the last of them is the
    // one the script keeps, and `on tick` already saw all sixteen.
    let many: Vec<_> = (0..20)
        .map(|i| damage_event(i, guest_layout::Datum::NONE, 0.5 + i as f32 / 100.0))
        .collect();
    m.put_u32(GAME_TIME + 8, TICK + 1);
    s.lock().pre_tick(&m);
    s.lock().post_tick(&m, &many);
    assert_eq!(global(&s, "hits"), 16);
    assert_eq!(global(&s, "at_tick"), 16);
    assert_eq!(global(&s, "last_amount"), data::to_fixed(0.65));
    assert_eq!(global(&s, "last_by"), -1, "no one is credited");

    // The next tick has none of them again.
    tick(&s, &mut m, 2);
    assert_eq!(global(&s, "hits"), 16);

    // Credit is the player slot; events that are not damage are ignored.
    let events = [
        sim_state::SimEvent::ActorKilled {
            unit: guest_layout::Datum(9),
            character: guest_layout::Datum(9),
            killer: guest_layout::Datum::NONE,
            weapon: 0,
        },
        damage_event(7, guest_layout::Datum(1), 0.25),
    ];
    m.put_u32(GAME_TIME + 8, TICK + 3);
    s.lock().pre_tick(&m);
    s.lock().post_tick(&m, &events);
    assert_eq!(global(&s, "hits"), 17);
    assert_eq!(global(&s, "last_by"), 1);
    assert_eq!(global(&s, "last_amount"), data::to_fixed(0.25));
}
