//! Zones, the roster's events and loadouts on the host
//! (`super::super::world::zones`, `super::super::world::loadout`), over
//! invented guest memory: two territory flags, two players with units
//! whose positions the test moves, and fake engine callees.

use engine::view::Datums;
use engine::view::objects::{Body, Header};

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
    s.lock().post_tick(m);
    work
}

fn global(s: &Arc<Shared>, name: &str) -> i32 {
    s.lock().running().unwrap().vm.global(name).unwrap()
}

const ZONES: &str = r#"
rules "Zones" 1
carrier slayer
label site: zones at least 1 = "territory"
global var entered: int = 0
global var left: int = 0
global var last: player? = none
global var held: int = 0
global var inside_now: int = 0
on enter(site z, o) {
    entered = entered + 1
    last = player_of(o)
}
on leave(site z, o) { left = left + 1 }
on tick {
    held = 0
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

const KIT: &str = r#"
rules "Kit" 1
carrier slayer
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
    let entry = move |cx: Cx| -> EntryFuture { Box::pin(run_work(cx, Arc::clone(&shared), work.clone())) };
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

#[test]
fn the_built_in_siege_is_hostable() {
    let ir = ::rules::compile(::rules::builtin::SIEGE).unwrap();
    assert_eq!(host_refusal(&ir), None);
}
