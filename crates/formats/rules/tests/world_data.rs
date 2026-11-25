//! Advanced data of the world API (docs/game/rules-world-api.md, "Advanced
//! data"): an object's health and shield, `on damage`, squad state and
//! tag reads, over the fake game and invented tag definitions.

use rules::caps::{self, Status, id};
use rules::fake::{FakeFields, FakeGame};
use rules::fields::{self, Field, Kind};
use rules::ir::Op;
use rules::ty::fixed;
use rules::verify::{Fault, unhostable};
use rules::{Event, Input, Refusal, Vm, build, compile, diagnose, verify};

const BOSS: &str = r#"
rules "Boss" 1
type slayer
global var hits: int = 0
global var last_amount: fixed = 0.0
global var shield_seen: fixed = 0.0
global var health_seen: fixed = 0.0
global var fighting: bool = false
global var strength: fixed = 0.0
global var alive: int = 0
on damage(o, by, amount) {
    hits = hits + 1
    last_amount = amount
    shield_seen = o.shield
    health_seen = o.health
    if by != none { score(by, 1) }
}
on tick {
    fighting = squad_fighting(squad(3))
    strength = squad_strength(squad(3))
    alive = squad_alive(squad(3))
}
"#;

const BOSS_OBJECT: u32 = 0x4001;

fn boss_game() -> FakeGame {
    let mut g = FakeGame::with_players(2);
    g.shield.insert(BOSS_OBJECT, fixed::ONE);
    g.health.insert(BOSS_OBJECT, fixed::ONE);
    g.squad_alive[3] = 4;
    g.squad_fighting[3] = true;
    g.squad_strength[3] = fixed::ONE / 2;
    g
}

fn damage(by: Option<u8>, amount: i32) -> Input {
    Input::Damage { object: BOSS_OBJECT, by, amount }
}

#[test]
fn on_damage_carries_the_object_the_credit_and_the_amount() {
    let (ir, _) = build(BOSS).unwrap();
    let mut vm = Vm::new(ir, 0, 0).unwrap();
    let mut g = boss_game();
    vm.begin_tick(1);
    let input = damage(Some(1), fixed::ONE / 4);
    assert!(g.apply(&input));
    vm.fire(&mut g, input);
    assert_eq!(vm.global("hits"), Some(1));
    assert_eq!(vm.global("last_amount"), Some(fixed::ONE / 4));
    // The fake game's shield took the damage first, and the handler read
    // the object's vitality after it.
    assert_eq!(vm.global("shield_seen"), Some(fixed::ONE * 3 / 4));
    assert_eq!(vm.global("health_seen"), Some(fixed::ONE));
    assert_eq!(g.calls_to("score"), [&vec![rules::vm::Arg::Player(Some(1)), rules::vm::Arg::Int(1)]]);
    // A hit credited to no one scores nothing.
    let input = damage(None, fixed::ONE / 2);
    g.apply(&input);
    vm.fire(&mut g, input);
    assert_eq!(vm.global("hits"), Some(2));
    assert_eq!(g.calls_to("score").len(), 1);
    // The next hit drains the rest of the shield into the body.
    let input = damage(Some(0), fixed::ONE);
    g.apply(&input);
    vm.fire(&mut g, input);
    assert_eq!(vm.global("shield_seen"), Some(0));
    assert_eq!(vm.global("health_seen"), Some(fixed::ONE / 4));
}

#[test]
fn an_object_that_is_gone_reads_zero_and_squads_read_their_tables() {
    let (ir, _) = build(BOSS).unwrap();
    let mut vm = Vm::new(ir, 0, 0).unwrap();
    let mut g = boss_game();
    vm.begin_tick(1);
    vm.update(&mut g);
    assert_eq!(vm.global("fighting"), Some(1));
    assert_eq!(vm.global("strength"), Some(fixed::ONE / 2));
    assert_eq!(vm.global("alive"), Some(4));
    vm.fire(&mut g, damage(None, 0));
    assert_eq!(vm.global("shield_seen"), Some(fixed::ONE));
    assert_eq!(vm.global("health_seen"), Some(fixed::ONE));
    vm.fire(&mut g, Input::Damage { object: 0x9999, by: None, amount: 0 });
    assert_eq!(vm.global("shield_seen"), Some(0));
    assert_eq!(vm.global("health_seen"), Some(0));
}

#[test]
fn a_damage_handler_costs_the_cap_of_sixteen_a_tick() {
    assert_eq!(Event::Damage.worst_per_tick(), 16);
    assert_eq!(Event::named("damage"), Some(Event::Damage));
    assert_eq!(Event::from_code(Event::Damage.code()), Some(Event::Damage));
    assert_eq!(Event::Damage.args(), [rules::Ty::Object, rules::Ty::Player, rules::Ty::Fixed]);
    let report = verify(&compile(BOSS).unwrap()).unwrap();
    assert!(report.fits());
}

#[test]
fn a_host_refuses_what_has_no_entry_behind_it() {
    // Health, shield and the tag reads have a host half.
    for e in [id::OBJECT_HEALTH, id::OBJECT_SHIELD, id::TAG_INT, id::TAG_FIXED] {
        assert_eq!(caps::entry(e, caps::API_VERSION).unwrap().status, Status::Named);
    }
    // Squad state past `squad_alive` is not traced: each is refused by
    // name. A script that reads a tag field is hostable.
    let strength_only = BOSS.replace("    fighting = squad_fighting(squad(3))\n", "");
    for (src, name) in [
        (BOSS, "squad_fighting"),
        (strength_only.as_str(), "squad_strength"),
    ] {
        let ir = compile(src).unwrap();
        assert_eq!(unhostable(&ir).map(|e| e.name), Some(name), "{name}");
    }
    assert_eq!(unhostable(&compile(TAGS).unwrap()), None);
}

const TAGS: &str = r#"
rules "Tags" 1
type slayer
global var shield: fixed = 0.0
global var grade: int = 0
global var whole: fixed = 0.0
on tick {
    shield = tag_fixed("objects/boss", "vitals.maximum_shield")
    grade = tag_int("objects/boss", "vitals.grade")
    whole = tag_fixed("objects/boss", "vitals.grade")
}
"#;

fn fields() -> FakeFields {
    FakeFields::default()
        .with("objects/boss", "vitals.maximum_shield", Field { offset: 0, kind: Kind::Float })
        .with("objects/boss", "vitals.grade", Field { offset: 6, kind: Kind::Int { bytes: 1, signed: false } })
}

fn tag_game() -> FakeGame {
    let mut g = FakeGame::with_players(1);
    g.tag_fields = fields();
    let mut bytes = 0.75f32.to_le_bytes().to_vec();
    bytes.extend([0, 0, 7, 0]);
    g.tag_data.insert("objects/boss".into(), bytes);
    g
}

fn run(src: &str, mut g: FakeGame) -> (Vec<Option<i32>>, FakeGame) {
    let ir = compile(src).unwrap();
    let mut vm = Vm::new(ir, 0, 5).unwrap();
    vm.begin_tick(1);
    vm.update(&mut g);
    let globals = ["shield", "grade", "whole"].iter().map(|n| vm.global(n)).collect();
    (globals, g)
}

#[test]
fn a_tag_read_is_the_field_of_the_tag_and_the_same_on_two_machines() {
    let (a, ga) = run(TAGS, tag_game());
    let (b, gb) = run(TAGS, tag_game());
    assert_eq!(a, [Some(fixed::ONE * 3 / 4), Some(7), Some(fixed::from_int(7))]);
    assert_eq!(a, b);
    assert_eq!(ga.reads, gb.reads);
    assert_eq!(ga.calls, gb.calls);
}

#[test]
fn a_field_the_definitions_lack_is_a_load_error_naming_it() {
    let ir = compile(TAGS).unwrap();
    assert!(fields::check(&ir, &fields()).is_empty());
    let none = fields::check(&ir, &FakeFields::default());
    // Once for each field the script names.
    let named: Vec<&str> = none.iter().map(|p| p.field.as_str()).collect();
    assert_eq!(named, ["vitals.maximum_shield", "vitals.grade"]);
    assert!(none[0].text.contains("\"objects/boss\""), "{}", none[0].text);
    assert!(none[0].text.contains("\"vitals.maximum_shield\""), "{}", none[0].text);
    assert!(none[0].text.contains("Tags"), "{}", none[0].text);
    // The right field of the wrong kind: a float read as an integer.
    let float_as_int = TAGS.replace(
        "tag_int(\"objects/boss\", \"vitals.grade\")",
        "tag_int(\"objects/boss\", \"vitals.maximum_shield\")",
    );
    let ir = compile(&float_as_int).unwrap();
    let problems = fields::check(&ir, &fields());
    assert_eq!(problems.len(), 1);
    assert!(problems[0].text.contains("tag_fixed"), "{}", problems[0].text);
    // A script with no tag read has nothing to resolve.
    assert!(fields::check(&compile(BOSS).unwrap(), &FakeFields::default()).is_empty());
}

#[test]
fn a_tag_read_names_a_valid_field_path_in_the_call() {
    let bad = TAGS.replace("vitals.grade\")\n    whole", "Vitals Grade\")\n    whole");
    assert_ne!(bad, TAGS);
    let c = diagnose(&bad);
    assert!(c.diagnostics.iter().any(|d| d.code == "field.path"), "{:#?}", c.diagnostics);
    assert!(compile(&bad).is_err());
}

#[test]
fn the_verifier_holds_a_tag_read_to_its_two_constants() {
    let ir = compile(TAGS).unwrap();
    let (hi, pc) = ir
        .handlers
        .iter()
        .enumerate()
        .find_map(|(h, hd)| {
            hd.code.iter().position(|o| matches!(o, Op::Call { entry, .. } if *entry == id::TAG_INT)).map(|p| (h, p))
        })
        .unwrap();
    assert!(verify(&ir).is_ok());

    // The field's constant is overwritten by a move: not directly before.
    let mut moved = ir.clone();
    let Op::Call { args, .. } = moved.handlers[hi].code[pc].clone() else { unreachable!() };
    moved.handlers[hi].code[pc - 1] = Op::Move { dst: args[1], src: args[1] };
    match verify(&moved) {
        Err(Refusal::Op { fault: Fault::FieldRead("tag_int"), .. }) => {}
        other => panic!("{other:?}"),
    }

    // A field that is no path.
    let mut bad = ir.clone();
    let Op::Const { value, .. } = bad.handlers[hi].code[pc - 1] else { unreachable!() };
    bad.strings[value as usize] = "Not A Path".into();
    match verify(&bad) {
        Err(Refusal::Op { fault: Fault::FieldPath { entry: "tag_int", .. }, .. }) => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn what_no_entry_backs_is_a_compile_error() {
    let head = "rules \"T\" 1\ntype slayer\n";
    // A tag value and the held weapon have no entry; a map's squad by
    // name has no label kind.
    for body in [
        "global var t: int = 0\non damage(o, by, amount) { t = o.tag }\n",
        "label wardens: squad = \"wardens\"\non tick { }\n",
    ] {
        assert!(compile(&format!("{head}{body}")).is_err(), "{body}");
    }
}
