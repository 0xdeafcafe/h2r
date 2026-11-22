//! The checker: one case per diagnostic code, every error reported (up to
//! fifty), declared labels in the label table, handlers by label, `is`
//! and `T?` narrowing, and the weapon tag group. See
//! `docs/game/rules-language-types.md`, "Diagnostics".

use rules::fake::FakeGame;
use rules::ir::{LabelKind, TagGroup, label_flags};
use rules::{Event, Input, Severity, Vm, builtin, compile, diagnose};

const HEAD: &str = "rules \"T\" 1\ncarrier slayer\n";

/// A script of `HEAD`, the `requires` line if any, and `body`.
fn script(requires: &str, body: &str) -> String {
    if requires.is_empty() {
        format!("{HEAD}{body}")
    } else {
        format!("{HEAD}requires {requires}\n{body}")
    }
}

/// The first diagnostic with `code`, and its severity; panics with every
/// diagnostic when there is none.
fn find(src: &str, code: &str) -> (Severity, String) {
    let c = diagnose(src);
    match c.diagnostics.iter().find(|d| d.code == code) {
        Some(d) => (d.severity, d.message.clone()),
        None => panic!("no {code} in {:#?}\nfor\n{src}", c.diagnostics),
    }
}

fn error(src: &str, code: &str) -> String {
    let (severity, message) = find(src, code);
    assert_eq!(severity, Severity::Error, "{code}: {message}");
    assert!(compile(src).is_err());
    message
}

fn warning(src: &str, code: &str) -> String {
    let (severity, message) = find(src, code);
    assert_eq!(severity, Severity::Warning, "{code}: {message}");
    assert!(diagnose(src).ir.is_some(), "{code}: a warning still compiles");
    message
}

// ---- one case per code ----

#[test]
fn label_codes() {
    let m = error(&script("", "label barrier: objects\non tick { for each barier b { } }\n"), "label.unknown");
    assert!(m.contains("Did you mean barrier?"), "{m}");
    let m = error(
        &script("objects.delete", "label barrier: objects\non tick { delete(barrier) }\n"),
        "label.set-as-one",
    );
    assert!(m.contains("barrier is a set of objects, and delete takes one"), "{m}");
    let m = error(&script("", "label core: object\non tick { for each core c { } }\n"), "label.one-as-set");
    assert!(m.contains("core is one object, not a set"), "{m}");
    let m = error(
        &script("", "label pad: object\non tick { for each players p where inside(p, pad) { } }\n"),
        "label.kind",
    );
    assert!(m.contains("declare it \"label pad: zone\""), "{m}");
    let m = error(&script("", "label start: spawns\non tick { for each start s { } }\n"), "label.spawns");
    assert!(m.contains("Pass it to spawn_areas"), "{m}");
    let m = error(&script("", "label hill: zones\non tick { if let z = hill[0] { } }\n"), "label.index");
    assert!(m.contains("Add \"numbered\""), "{m}");
    let m = error(
        &script("", "label cap: zones numbered 0..1\non tick { if let z = cap[2] { } }\n"),
        "label.index",
    );
    assert!(m.contains("outside \"numbered 0..1\""), "{m}");
    let m = error(
        &script("", "label a: zones = \"x\"\nlabel b: zones = \"x\"\non tick {\n    for each a z { }\n    for each b y { }\n}\n"),
        "label.same-text",
    );
    assert!(m.contains("both name the map label \"x\""), "{m}");
    warning(&script("", "label a: zones\n"), "label.unused");
}

#[test]
fn null_codes() {
    let m = error(
        &script("", "global var n: int = 0\non kill(victim, killer) { n = killer.kills }\n"),
        "null.use",
    );
    assert!(m.contains("killer may be none here"), "{m}");
    let m = error(&script("", "on kill(victim, killer) { score(killer, 1) }\n"), "null.arg");
    assert!(m.contains("score takes a player, and killer may be none here"), "{m}");
    let m = error(&script("", "global var j: player = none\n"), "null.var");
    assert!(m.contains("so its type is player?"), "{m}");
    error(&script("", "global var j: player\n"), "null.var");
    warning(&script("", "on spawn(p) { if p != none { score(p, 1) } }\n"), "null.never");
}

#[test]
fn type_codes() {
    let m = error(
        &script("player.weapons", "table t: (w: tag) = [(\"a/b\")]\non spawn(p) { give(p, t[0].w) }\n"),
        "tag.group",
    );
    assert!(m.contains("give takes a weapon"), "{m}");
    let m = warning(
        &script("objects.delete", "label goal: zone\non tick { delete(goal) }\n"),
        "zone.delete",
    );
    assert!(m.contains("A flag cannot be deleted"), "{m}");
    let m = error(&script("", "on tick { let x = 1 + true }\n"), "type.mismatch");
    assert!(m.contains("expected bool, found int"), "{m}");
    let m = error(&script("", "on tick { score(nobody, 1) }\n"), "name.unknown");
    assert!(m.contains("unknown name nobody"), "{m}");
}

#[test]
fn event_and_capability_codes() {
    let m = error(&script("", "on spawn(p, q) { }\n"), "event.args");
    assert!(m.contains("on spawn gives 1 argument (player); this handler names 2."), "{m}");
    let m = error(&script("", "label crate: objects\non enter(crate, o) { }\n"), "event.label");
    assert!(m.contains("crate is an object; on enter takes a zone label."), "{m}");
    let m = error(&script("", "label goal: zone\non kill(goal v, k) { }\n"), "event.label");
    assert!(m.contains("on kill takes no label"), "{m}");
    let m = error(&script("", "on spawn(p) { set_team(p, team 1) }\n"), "cap.missing");
    assert!(m.contains("set_team needs capability player.team"), "{m}");
    warning(&script("player.team", "on spawn(p) { }\n"), "cap.unused");
}

#[test]
fn declaration_syntax_and_limit_codes() {
    error(&script("", "on tick {\n"), "syntax");
    error(&script("", "label a: zones\nlabel a: zones\n"), "decl");
    error(
        &script("", "option a: int 0..65535 = 0\noption b: int 0..65535 = 0\noption c: bool = false\n"),
        "limit",
    );
    warning(&script("", "label goal: zone\non enter(goal, o) { let goal = 1 }\n"), "name.shadow");
}

// ---- every error, in order ----

#[test]
fn five_errors_are_five_diagnostics_in_source_order() {
    let src = script(
        "",
        "global var j: player = none
on kill(victim, killer) {
    score(killer, 1)
    score(nobody, 1)
    let x = 1 + true
}
on spawn(p, q) { }
",
    );
    let c = diagnose(&src);
    assert!(c.ir.is_none());
    let codes: Vec<&str> = c.errors().map(|d| d.code).collect();
    assert_eq!(codes, ["null.var", "null.arg", "name.unknown", "type.mismatch", "event.args"]);
    let lines: Vec<u32> = c.errors().map(|d| d.pos.line).collect();
    assert_eq!(lines, [3, 5, 6, 7, 9]);
    // `compile` gives the first.
    assert_eq!(compile(&src).unwrap_err().line, 3);
}

#[test]
fn errors_stop_at_fifty() {
    let body: String = (0..60).map(|i| format!("    score(nobody{i}, 1)\n")).collect();
    let src = script("", &format!("on tick {{\n{body}}}\n"));
    assert_eq!(diagnose(&src).errors().count(), 50);
}

// ---- labels ----

#[test]
fn the_label_table_carries_the_declarations_in_order() {
    let ir = compile(builtin::SIEGE).unwrap();
    let texts: Vec<&str> = ir.labels.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, ["barrier", "attack", "defend", "zone", "core", "goal"]);
    let barrier = &ir.labels[0];
    assert_eq!((barrier.kind, barrier.flags, barrier.min, barrier.max), (LabelKind::Object, label_flags::NUMBERED, 0, 256));
    let attack = &ir.labels[1];
    assert_eq!(attack.kind, LabelKind::Spawn);
    assert_eq!(attack.flags, label_flags::NUMBERED | label_flags::RANGE);
    assert_eq!((attack.lo, attack.hi), (0, 2));
    let capture = &ir.labels[3];
    assert_eq!((capture.kind, capture.lo, capture.hi), (LabelKind::Zone, 0, 1));
    let core = &ir.labels[4];
    assert_eq!((core.kind, core.flags, core.min, core.max), (LabelKind::Object, label_flags::SINGLE, 1, 1));
    assert_eq!(ir.labels[5].kind, LabelKind::Zone);

    let src = script(
        "",
        "label bonus: object?
label crates: objects at least 2
label pads: zones exactly 3
label walls: objects 2..4
label bases: zones per team
on tick {
    if let b = bonus { }
    for each crates c { }
    for each pads p { }
    for each walls w { }
    for each bases z { }
}
",
    );
    let ir = compile(&src).unwrap();
    let got: Vec<(u16, u16, u8)> = ir.labels.iter().map(|l| (l.min, l.max, l.flags)).collect();
    assert_eq!(
        got,
        [
            (0, 1, label_flags::SINGLE),
            (2, 256, 0),
            (3, 3, 0),
            (2, 4, 0),
            (0, 256, label_flags::PER_TEAM),
        ]
    );
    let ctf = compile(builtin::CTF).unwrap();
    assert!(ctf.labels.iter().all(|l| l.max == 16 && l.kind == LabelKind::Zone));
}

#[test]
fn handlers_by_label_are_one_handler_and_skip_unlabelled_things() {
    let src = script(
        "",
        "label goal: zone
label base: zone
global var g: int = 0
global var b: int = 0
on enter(goal, o) { g = g + 1 }
on enter(base z, o) { b = b + 1 }
",
    );
    let ir = compile(&src).unwrap();
    assert_eq!(ir.handlers.iter().filter(|h| h.event == Event::Enter).count(), 1);
    let mut g = FakeGame::with_players(1);
    g.label("goal", 10, 0, 0);
    g.label("base", 11, 0, 1);
    let mut vm = Vm::new(ir, 0, 0).unwrap();
    vm.begin_tick(1);
    vm.fire(&mut g, Input::Enter { zone: 10, object: 5 });
    vm.fire(&mut g, Input::Enter { zone: 11, object: 5 });
    vm.fire(&mut g, Input::Enter { zone: 11, object: 6 });
    // A zone with no declared label is neither, though `goal` is label 0.
    vm.fire(&mut g, Input::Enter { zone: 99, object: 5 });
    assert_eq!((vm.global("g"), vm.global("b")), (Some(1), Some(2)));
}

#[test]
fn is_narrows_an_object_to_its_labels_zone() {
    let with = script(
        "",
        "label goal: zone
global var n: int = 0
on drop(p, o) { if o is goal and inside(p, o) { n = n + 1 } }
",
    );
    let without = with.replace("o is goal and ", "");
    let m = error(&without, "type.mismatch");
    assert!(m.contains("inside takes a zone, and o is an object"), "{m}");
    let ir = compile(&with).unwrap();
    let mut g = FakeGame::with_players(1);
    g.label("goal", 10, 0, 0);
    g.inside.extend([(0, 10), (0, 99)]);
    let mut vm = Vm::new(ir, 0, 0).unwrap();
    vm.begin_tick(1);
    vm.fire(&mut g, Input::Drop(0, 10));
    vm.fire(&mut g, Input::Drop(0, 99));
    assert_eq!(vm.global("n"), Some(1));
}

#[test]
fn a_numbered_index_finds_its_member_or_none() {
    let src = script(
        "",
        "label hill: zones numbered
global var found: int = -1
on tick { if let z = hill[1] { found = z.id } else { found = -2 } }
",
    );
    let ir = compile(&src).unwrap();
    let mut g = FakeGame::with_players(1);
    g.label("hill", 20, 0, 0);
    let mut vm = Vm::new(ir.clone(), 0, 0).unwrap();
    vm.begin_tick(1);
    vm.update(&mut g);
    assert_eq!(vm.global("found"), Some(-2));
    g.label("hill", 21, 1, 1);
    let mut vm = Vm::new(ir, 0, 0).unwrap();
    vm.begin_tick(1);
    vm.update(&mut g);
    assert_ne!(vm.global("found"), Some(-2));
}

// ---- T? ----

#[test]
fn checks_against_none_narrow() {
    for body in [
        "on kill(victim, killer) { if killer != none { score(killer, 1) } }",
        "on kill(victim, killer) { if killer == none { } else { score(killer, 1) } }",
        "on kill(victim, killer) { if killer == none or killer == victim { } else { score(killer, 1) } }",
        "on kill(victim, killer) { if not (killer == none) and killer.team == victim.team { score(killer, 1) } }",
        "on kill(victim, killer) { if let k = killer and k != victim { score(k, 1) } }",
        "fn reward(p: player?) { if p != none { score(p, 1) } }\non kill(victim, killer) { reward(killer) }",
        "on round_start { for each players p where p.alive { score(p, 1) } }",
        "on round_start { let heir: player? = none\n if heir != none { score(heir, 1) } }",
    ] {
        let src = script("", &format!("{body}\n"));
        let c = diagnose(&src);
        assert!(c.ir.is_some(), "{body}: {:#?}", c.diagnostics);
    }
    // A global is not narrowed: another handler may change it.
    let src = script("", "global var j: player? = none\non tick { if j != none { score(j, 1) } }\n");
    error(&src, "null.arg");
    // `let x = none` has no type.
    error(&script("", "on tick { let x = none }\n"), "type.mismatch");
}

// ---- tags ----

#[test]
fn a_tag_named_as_a_weapon_is_of_the_weapon_group() {
    let ir = compile(builtin::GUN_GAME).unwrap();
    assert!(!ir.tags.is_empty());
    assert!(ir.tags.iter().all(|t| t.group == TagGroup::Weapon));
    let src = script(
        "objects.create, player.weapons",
        "label pad: object
palette \"kit\" = [\"objects/a\"]
on spawn(p) {
    create(\"objects/b\", pad)
    give(p, \"objects/b\")
    create(\"objects/a\", pad)
}
",
    );
    let ir = compile(&src).unwrap();
    let groups: Vec<(&str, TagGroup)> = ir.tags.iter().map(|t| (t.path.as_str(), t.group)).collect();
    assert_eq!(groups, [("objects/a", TagGroup::Any), ("objects/b", TagGroup::Weapon)]);
}
