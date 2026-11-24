//! The checker: one case per diagnostic code, every error reported (up to
//! fifty), declared labels in the label table, handlers by label, `is`
//! and `T?` narrowing, the weapon tag group, and time, constants, `match`
//! and value functions, and the settings block, the quick line and the
//! scorer. See `docs/game/rules-language-types.md`, "Stronger typing" and
//! "Diagnostics", and `docs/game/rules-variants.md`.

use rules::fake::FakeGame;
use rules::ir::{LabelKind, Quick, Setting, TagGroup, label_flags};
use rules::types::Type;
use rules::{Event, Input, Severity, Vm, builtin, compile, diagnose, settings};

/// Runs one tick of `src` with its default options and gives the global
/// `name`.
fn tick_global(src: &str, name: &str) -> Option<i32> {
    let ir = compile(src).unwrap();
    let options = ir.default_options();
    let mut vm = Vm::new(ir, options, 0).unwrap();
    let mut g = FakeGame::with_players(1);
    vm.begin_tick(1);
    vm.update(&mut g);
    vm.global(name)
}

const HEAD: &str = "rules \"T\" 1\ntype slayer\n";

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
        "const.range",
    );
    assert!(m.contains("outside \"numbered 0..1\""), "{m}");
    let m = error(
        &script("", "label a: zones = \"x\"\nlabel b: zones = \"x\"\non tick {\n    for each a z { }\n    for each b y { }\n}\n"),
        "label.same-text",
    );
    assert!(m.contains("both name the map label \"x\""), "{m}");
    warning(&script("", "label a: zones\non tick { }\n"), "label.unused");
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

#[test]
fn text_hole_codes() {
    let msg = |body: &str| script("hud.message", body);
    let m = error(&msg("on kill(victim, killer) { let n = 2\nmessage(victim, \"{q} got you\") }\n"), "text.hole");
    assert_eq!(m, "{q} in this message is not a name here. Names in scope: victim, killer, n.");
    let m = error(&msg("on pickup(p, o) { message(p, \"{o} is yours\") }\n"), "text.hole");
    assert_eq!(
        m,
        "an object can't be shown in a message. A message shows a player, a team, an int, a time or an enum."
    );
    let m = error(&msg("on kill(victim, killer) { message(victim, \"{killer} got you\") }\n"), "text.hole");
    assert!(m.contains("killer may be none here"), "{m}");
    let m = error(&msg("on spawn(p) { message(p, \"{p scored\") }\n"), "text.hole");
    assert!(m.contains("this { opens a hole with no }"), "{m}");
    let m = error(&msg("on spawn(p) { message(p, \"p} scored\") }\n"), "text.hole");
    assert!(m.contains("this } closes no hole"), "{m}");
    let m = error(&msg("on spawn(p) { message(p, \"{p.team} scored\") }\n"), "text.hole");
    assert!(m.contains("{p.team} is not a name"), "{m}");
    let m = error(&msg("on spawn(p) { message(p, \"{p}{p}{p}{p}{p}\") }\n"), "text.hole");
    assert!(m.contains("at most 4 values, and this one has 5"), "{m}");
    // The hole is reported where it is: column 29 is the `{` of `{q}`.
    let c = diagnose(&msg("on spawn(p) { message(p, \"a {q}\") }\n"));
    let d = c.diagnostics.iter().find(|d| d.code == "text.hole").unwrap();
    assert_eq!((d.pos.line, d.pos.col), (4, 29));
    // Narrowed, a handle that may be none shows; a doubled brace is a brace.
    let ok = msg("on kill(victim, killer) { if let k = killer { message(victim, \"{k} got you {{x}}\") } }\n");
    assert!(diagnose(&ok).diagnostics.is_empty(), "{:#?}", diagnose(&ok).diagnostics);
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
    // Each kind of declaration once: numbered objects, numbered spawns in a
    // range, a renamed numbered zone, a single object and a single zone
    // (unused labels only warn).
    let ir = compile(&script(
        "",
        "const last_phase = 2
label barrier: objects numbered
label attack: spawns numbered 0..last_phase
label defend: spawns numbered 0..last_phase
label capture: zones numbered 0..1 = \"zone\"
label core: object
label goal: zone
on tick { }
",
    ))
    .unwrap();
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

// ---- time ----

#[test]
fn unit_codes() {
    // The page's texts, word for word.
    let m = error(
        &script(
            "",
            "player var held: time = 0t\non tick { for each players p { if p.held >= 1 { score(p, 1) } } }\n",
        ),
        "unit.mix",
    );
    assert_eq!(m, "can't compare time with int in \"p.held >= 1\". Write 1s for a second or 30t for ticks.");
    let m = error(
        &script(
            "",
            "option wave_seconds: int 3..15 = 8\nglobal var wave: timer\non round_start { wave = wave_seconds }\n",
        ),
        "unit.mix",
    );
    assert_eq!(
        m,
        "wave is a timer and takes time; wave_seconds is an int. Write seconds(wave_seconds), or declare the option as time."
    );
    let m = error(&script("", "global var t: time = 0t\non tick { t = 0.01s }\n"), "unit.literal");
    assert_eq!(m, "0.01s is not a whole number of ticks (30 a second).");
    // A time option is declared in whole seconds.
    let m = error(&script("", "option t: time 1s..10s = 1.5s\n"), "unit.literal");
    assert!(m.contains("whole seconds"), "{m}");
    // A time takes no bare int, and time * time has no meaning.
    let m = error(&script("", "global var t: time = 0t\non tick { t = 5 }\n"), "unit.mix");
    assert!(m.contains("Write 5s for seconds or 5t for ticks"), "{m}");
    error(&script("", "global var t: time = 0t\non tick { t = t * t }\n"), "unit.mix");
    let m = error(&script("", "global var n: int = 0\non tick { n = now() }\n"), "unit.mix");
    assert!(m.contains("is time. Write whole_seconds("), "{m}");
}

#[test]
fn time_counts_ticks() {
    // The page's design text compiles.
    let src = script(
        "",
        "label zone: zone
option capture_time: time 5s..60s = 20s
global var opened: bool = false
player var held: time = 0t
on tick {
    if ticks_held(zone, team 0) >= capture_time { opened = true }
    for each players p {
        p.held = p.held + 1t
        if p.held >= 1s { score(p, 1) }
    }
}
",
    );
    assert!(diagnose(&src).ir.is_some(), "{:#?}", diagnose(&src).diagnostics);
    for (expr, want) in [
        ("90t / 1s", 3),
        ("whole_seconds(95t)", 3),
        ("ticks(2s)", 60),
        ("ticks(max(1s, 2s) * 2 - 1.5s)", 75),
        ("ticks(seconds(4) / 2)", 60),
        ("ticks(-1t)", -1),
    ] {
        let src = script("", &format!("global var n: int = 0\non tick {{ n = {expr} }}\n"));
        assert_eq!(tick_global(&src, "n"), Some(want), "{expr}");
    }
}

#[test]
fn a_time_option_is_held_as_an_int_option_and_labelled_in_seconds() {
    let int = compile(&script("", "option hold: int 5..60 = 20 \"Capture time\"\non tick { }\n")).unwrap();
    let time = compile(&script("", "option hold: time 5s..60s = 20s \"Capture time\"\non tick { }\n")).unwrap();
    let (a, b) = (&int.options[0], &time.options[0]);
    assert_eq!((a.ty, a.lo, a.hi, a.default, a.bits()), (b.ty, b.lo, b.hi, b.default, b.bits()));
    assert_eq!(int.default_options(), time.default_options());
    assert_eq!(int.strings[a.label as usize], "Capture time");
    assert_eq!(time.strings[b.label as usize], "Capture time (seconds)");
    // With no label, the name carries the unit.
    let bare = compile(&script("", "option hold: time 5s..60s = 20s\non tick { }\n")).unwrap();
    assert_eq!(bare.strings[bare.options[0].label as usize], "hold (seconds)");
    // The record holds seconds; the script reads ticks.
    let src = script("", "option hold: time 5s..60s = 20s\nglobal var t: time = 0t\non tick { t = hold }\n");
    assert_eq!(tick_global(&src, "t"), Some(600));
    // A constant stands where a literal does.
    let src = script(
        "",
        "const hold = 20s\noption h: time 5s..60s = hold\nglobal var t: time = hold\non tick { t = t + h }\n",
    );
    assert_eq!(tick_global(&src, "t"), Some(1200));
}

// ---- constants ----

#[test]
fn const_codes() {
    let rows: Vec<String> = (0..12).map(|i| format!("({i})")).collect();
    let m = error(
        &script(
            "",
            &format!(
                "table ladder: (n: int) = [{}]\nglobal var x: int = 0\non tick {{ x = ladder[12].n }}\n",
                rows.join(", ")
            ),
        ),
        "const.range",
    );
    assert_eq!(m, "ladder[12]: ladder has 12 rows, 0 to 11.");
    let m = error(
        &script(
            "",
            "const last = 1\nconst over = last + 1\nlabel capture: zones numbered 0..last\non tick { if let z = capture[over] { } }\n",
        ),
        "const.range",
    );
    assert_eq!(m, "capture[2] is outside \"numbered 0..1\".");
    let m = error(&script("", "const big = 2147483647 + 1\n"), "const.range");
    assert!(m.contains("\"big\""), "{m}");
    let m = error(&script("", "const step = 10 / (2 - 2)\n"), "const.zero");
    assert_eq!(m, "division by zero in the constant \"step\".");
    // A constant of a constant that refers back is refused, as is a name
    // that is not a constant.
    error(&script("", "const a = b\nconst b = a\n"), "decl");
    error(&script("", "global var g: int = 0\nconst a = g\n"), "decl");
}

#[test]
fn constants_fold_wherever_a_literal_stands() {
    let src = script(
        "",
        "const last_phase = 2
const hold = 20s
const most = last_phase * 2
table t: (n: int) = [(1), (2), (3)]
const rows = len(t)
label walls: objects at most most
global var n: int = rows + last_phase
global var h: time = hold
on tick {
    for each walls w { }
    n = n + most
    h = h + hold / 2
}
",
    );
    let ir = compile(&src).unwrap();
    assert_eq!(ir.labels[0].max, 4);
    assert_eq!(tick_global(&src, "n"), Some(5 + 4));
    assert_eq!(tick_global(&src, "h"), Some(600 + 300));
    // A let that shadows a constant is a warning.
    warning(&script("", "const k = 1\non tick { let k = 2 }\n"), "name.shadow");
}

// ---- match ----

#[test]
fn match_codes() {
    let src = script(
        "",
        "enum Role { none, vip, guard }
player var role: Role = Role.none
on spawn(p) {
    match p.role {
        Role.vip { score(p, 1) }
        Role.none { }
    }
}
",
    );
    let m = error(&src, "match.missing");
    assert_eq!(m, "match on Role has no arm for Role.guard. Add \"Role.guard { }\" or \"else { }\".");
    // An else arm covers the rest; a variant twice is refused.
    assert!(compile(&src.replace("Role.none { }", "else { }")).is_ok());
    error(&src.replace("Role.none { }", "Role.vip { }\n        else { }"), "decl");
}

#[test]
fn match_runs_the_arm_of_the_value() {
    let src = script(
        "",
        "enum Mode { a, b, c, d }
global var m: Mode = Mode.c
global var out: int = 0
on tick {
    match m {
        Mode.a { out = 1 }
        Mode.b, Mode.c { out = 2 }
        else { out = 3 }
    }
}
",
    );
    assert_eq!(tick_global(&src, "out"), Some(2));
    assert_eq!(tick_global(&src.replace("= Mode.c", "= Mode.a"), "out"), Some(1));
    assert_eq!(tick_global(&src.replace("= Mode.c", "= Mode.d"), "out"), Some(3));
}

#[test]
fn a_script_names_no_engine() {
    // No carrier values, and no engine index to compare them with.
    let src = script("", "global var out: int = 0\non tick { if variant.engine == 2 { out = 1 } }\n");
    let m = error(&src, "name.unknown");
    assert_eq!(m, "variant has no field engine");
    let src = script("", "global var out: int = 0\non tick { if out == carrier.slayer { out = 1 } }\n");
    let m = error(&src, "syntax");
    assert!(m.contains("a script names no engine"), "{m}");
}

// ---- value functions ----

#[test]
fn value_functions_are_inlined_and_checked() {
    let src = script(
        "",
        "fn rival(a: player, b: player) -> bool = (variant.team_play and a.team != b.team) or (not variant.team_play and a != b)
global var n: int = 0
on kill(victim, killer) {
    if let k = killer and rival(k, victim) { n = n + 1 }
}
",
    );
    assert!(compile(&src).is_ok(), "{:#?}", diagnose(&src).diagnostics);
    let m = error(&src.replace("if let k = killer and rival(k, victim)", "if rival(killer, victim)"), "null.arg");
    assert!(m.starts_with("rival takes a player, and killer may be none here."), "{m}");
    let src = script(
        "",
        "fn twice(n: int) -> int = n * 2\nfn later(t: time) -> time = t + 1s\nglobal var x: int = 0\non tick { x = twice(21) + ticks(later(1t)) }\n",
    );
    assert_eq!(tick_global(&src, "x"), Some(42 + 31));
    // A value function is used for its value; a block function has none.
    error(&script("", "fn twice(n: int) -> int = n * 2\non tick { twice(1) }\n"), "type.mismatch");
    error(&script("", "fn f(n: int) -> int { }\n"), "decl");
    error(&script("", "fn f(n: int) = n\n"), "decl");
}

// ---- the built-ins ----

#[test]
fn the_builtins_use_time_constants_and_match() {
    assert!(builtin::SIEGE.contains("option capture_time: time"));
    assert!(builtin::ODDBALL.contains("player var held: time"));
    assert!(builtin::VIP.contains("match victim.role"));
    assert!(builtin::KING.contains("fn rival(a: player, b: player) -> bool ="));
    assert!(builtin::CTF.contains("const assault = 0") && builtin::ASSAULT.contains("const assault = 1"));
    // A time option's label carries its unit in the blob.
    let siege = compile(builtin::SIEGE).unwrap();
    let labels: Vec<&str> = siege.options.iter().map(|o| siege.strings[o.label as usize].as_str()).collect();
    assert_eq!(labels, ["Phase time (seconds)", "Capture time (seconds)", "Sites", "Respawn wave (seconds)"]);
}

// ---- settings, quick marks and the scorer ----

/// A script named V of type `ty`, with `lines` after its header.
fn riding(ty: &str, lines: &str) -> String {
    format!("rules \"V\" 1\ntype {ty}\n{lines}")
}

/// A settings-only script of the retail type `ty` that uses its built-in
/// game, with a `settings` block of `lines`.
fn carried(ty: &str, lines: &str) -> String {
    riding(ty, &format!("uses {ty}\nsettings {{\n{lines}}}\n"))
}

/// The table index of a setting.
fn at(name: &str) -> u16 {
    settings::named(name).unwrap().0
}

const SWAT: &str = "rules \"SWAT\" 1
type slayer
uses slayer
about \"No shields, no motion sensor, battle rifles and magnums. One headshot kills. 50 points to win.\"
quick score_limit, starting_weapon, secondary_weapon

settings {
    team_play = true
    score_limit = 50
    shields = none
    motion_sensor = false
    starting_weapon = battle_rifle
    secondary_weapon = magnum
    weapons_on_map = none
    grenades_on_map = false
    starting_grenades = false
    overshield_pickups = false
    camo_pickups = false
}
";

#[test]
fn the_settings_block_lowers_to_settings_in_table_order() {
    let c = diagnose(SWAT);
    assert!(c.diagnostics.is_empty(), "{:#?}", c.diagnostics);
    let ir = c.ir.unwrap();
    assert_eq!(ir.header.uses, Some(Type::Slayer));
    assert_eq!(ir.header.points, "Kills");
    assert!(ir.header.about.starts_with("No shields, no motion sensor"));
    assert!(ir.handlers.is_empty() && ir.slots.is_empty() && ir.options.is_empty());
    let mut want = vec![
        ("team_play", 1),
        ("score_limit", 50),
        ("shields", 1),
        ("motion_sensor", 0),
        ("starting_weapon", 3),
        ("secondary_weapon", 4),
        ("weapons_on_map", 1),
        ("grenades_on_map", 0),
        ("starting_grenades", 0),
        ("overshield_pickups", 0),
        ("camo_pickups", 0),
    ];
    want.sort_by_key(|(n, _)| at(n));
    let got: Vec<(&str, i32)> = ir
        .settings
        .iter()
        .map(|s| (settings::TABLE[s.index as usize].name, s.value))
        .collect();
    assert_eq!(got, want);
    assert_eq!(
        ir.quick,
        [at("score_limit"), at("starting_weapon"), at("secondary_weapon")].map(Quick::Setting)
    );
    assert!(rules::load(&ir.encode()).is_ok());
}

#[test]
fn each_kind_takes_its_values() {
    let src = carried(
        "slayer",
        "    time_limit = 300s\n    respawn_time = 90t\n    score_limit = none\n    light_vehicle = none\n    \
         weapons_on_map = 12\n    gravity = 50\n    unlimited_grenades = true\n    shields = overshields\n",
    );
    let ir = compile(&src).unwrap();
    let mut want = vec![
        Setting { index: at("time_limit"), value: 300 },
        Setting { index: at("respawn_time"), value: 3 },
        Setting { index: at("score_limit"), value: 0 },
        Setting { index: at("light_vehicle"), value: 7 },
        Setting { index: at("weapons_on_map"), value: 12 },
        Setting { index: at("gravity"), value: 50 },
        Setting { index: at("unlimited_grenades"), value: 1 },
        Setting { index: at("shields"), value: 2 },
    ];
    want.sort_by_key(|s| s.index);
    assert_eq!(ir.settings, want);
    // A constant stands where a literal does, in a script that scores.
    let src = riding("slayer", "const hold = 60s\nsettings { time_limit = hold * 2 }\non tick { }\n");
    assert_eq!(compile(&src).unwrap().settings, [Setting { index: at("time_limit"), value: 120 }]);
}

#[test]
fn setting_codes() {
    let m = error(&carried("slayer", "    score_limt = 5\n"), "setting.unknown");
    assert_eq!(m, "no setting named score_limt. Did you mean score_limit?");
    let m = error(&carried("slayer", "    lives = 1\n    lives = 2\n"), "setting.twice");
    assert_eq!(m, "lives is set twice; it is first set on line 5");
    let value = |line: &str| error(&carried("slayer", &format!("    {line}\n")), "setting.value");
    let m = value("time_limit = 300");
    assert!(m.starts_with("time_limit is a time: write 300s"), "{m}");
    assert!(value("motion_sensor = 1").starts_with("motion_sensor is on or off"));
    let m = value("starting_weapon = laser");
    assert!(m.starts_with("starting_weapon has no value laser."), "{m}");
    assert!(m.contains("battle_rifle"), "{m}");
    assert!(value("rounds = 9").starts_with("rounds is 0 to 6, and 9 is outside"));
    assert!(value("lives = none").starts_with("lives takes no none"));
    assert!(value("respawn_time = 1.5s").contains("not a whole number of seconds"));
    assert!(value("gravity = 5").starts_with("gravity is 10 to 400"));
}

#[test]
fn a_setting_of_another_game_is_refused() {
    let m = error(&carried("slayer", "    juggernaut_movement = fast\n"), "setting.type");
    assert_eq!(m, "juggernaut_movement is a Juggernaut setting; this script's record is Slayer's");
    let ir = compile(&carried("juggernaut", "    juggernaut_movement = fast\n")).unwrap();
    assert_eq!(ir.settings, [Setting { index: at("juggernaut_movement"), value: 2 }]);
    let m = error(&carried("koth", "    ctf_flag_type = 1\n"), "setting.type");
    assert!(m.contains("a Capture the Flag and Assault setting"), "{m}");
    assert!(compile(&carried("assault", "    ctf_flag_type = 1\n")).is_ok());
    // A party game that uses Juggernaut's rules takes Juggernaut's settings.
    let src = riding("party", "uses juggernaut\nsettings { juggernaut_movement = fast }\n");
    assert!(compile(&src).is_ok(), "{:#?}", diagnose(&src).diagnostics);
    // The blob of another game's setting is refused by the verifier too.
    let mut ir = ir;
    ir.header.game_type = Type::Slayer;
    ir.header.uses = Some(Type::Slayer);
    assert!(matches!(rules::load(&ir.encode()), Err(rules::Refusal::Setting { .. })));
}

#[test]
fn score_limit_is_one_name_on_every_retail_type() {
    let row = &settings::TABLE[at("score_limit") as usize];
    for t in rules::types::TABLE.iter().filter(|t| t.ty.is_retail()) {
        let (name, engine) = (t.name, t.engine);
        let ir = compile(&carried(name, "    score_limit = 3\n")).unwrap();
        assert_eq!(ir.settings, [Setting { index: at("score_limit"), value: 3 }], "{name}");
        assert_eq!(ir.header.engine(), engine, "{name}");
        assert_eq!(row.setting(engine), settings::score_setting(engine), "{name}");
        assert!(row.setting(engine).is_some(), "{name}");
    }
}

#[test]
fn a_sixth_quick_mark_is_refused() {
    let five = "score_limit, time_limit, lives, respawn_time, rounds";
    let ir = compile(&riding("slayer", &format!("uses slayer\nquick {five}\n"))).unwrap();
    assert_eq!(ir.quick.len(), 5);
    let m = error(&riding("slayer", &format!("uses slayer\nquick {five}, shields\n")), "quick.many");
    assert_eq!(m, "Quick Options has five rows; this line names 6");
}

#[test]
fn a_quick_mark_names_a_setting_or_an_option() {
    // A setting need not be in the block to be marked.
    let src = riding(
        "slayer",
        "quick time_limit, bounce_points\noption bounce_points: int 1..5 = 1\non kill(v, k) { }\n",
    );
    let ir = compile(&src).unwrap();
    assert_eq!(ir.quick, [Quick::Setting(at("time_limit")), Quick::Option(0)]);
    let m = error(&riding("slayer", "uses slayer\nquick bounce_points\n"), "quick.unknown");
    assert_eq!(m, "bounce_points is no setting and no option of this script");
    let m = error(&riding("slayer", "uses slayer\nquick live\n"), "quick.unknown");
    assert!(m.ends_with("Did you mean lives?"), "{m}");
    let m = error(&riding("slayer", "uses slayer\nquick gravity\n"), "quick.unknown");
    assert!(m.contains("More Options"), "{m}");
    let m = error(&riding("slayer", "uses slayer\nquick lives, lives\n"), "quick.twice");
    assert_eq!(m, "lives is marked twice");
    let m = error(&riding("slayer", "uses slayer\nquick juggernaut_movement\n"), "setting.type");
    assert!(m.starts_with("juggernaut_movement is a Juggernaut setting"), "{m}");
    // A setting and an option share one namespace.
    let m = error(&riding("slayer", "option lives: int 1..5 = 1\non kill(v, k) { }\n"), "option.setting");
    assert!(m.contains("option lives has a setting's name"), "{m}");
}

#[test]
fn a_uses_script_takes_no_handlers() {
    for item in [
        "on kill(v, k) { }",
        "option points: int 1..5 = 1",
        "global var n: int = 0",
        "fn f() { }",
    ] {
        let src = riding("slayer", &format!("uses slayer\n{item}\n"));
        let m = error(&src, "uses.handlers");
        assert!(m.contains("with uses slayer the built-in game scores"), "{item}: {m}");
    }
    // Its header lines, settings and quick are its whole script.
    let src = riding(
        "research",
        "uses slayer\nrequires hud.superbounce\npoints \"Bounces\"\nabout \"Bounce.\"\nquick lives\nsettings { lives = 3 }\n",
    );
    let c = diagnose(&src);
    assert!(c.diagnostics.is_empty(), "{:#?}", c.diagnostics);
    let ir = c.ir.unwrap();
    assert_eq!((ir.header.game_type, ir.header.uses), (Type::Research, Some(Type::Slayer)));
    assert_eq!((ir.header.points.as_str(), ir.header.about.as_str()), ("Bounces", "Bounce."));
    assert_eq!(ir.header.engine(), 2);
    // `uses` names a built-in game: a retail type.
    let m = error(&riding("party", "uses party\n"), "name.unknown");
    assert_eq!(m, "uses names a built-in game: slayer, ctf, assault, oddball, koth, juggernaut, territories");
    let m = error(&riding("party", "uses king\n"), "name.unknown");
    assert!(m.starts_with("uses names a built-in game"), "{m}");
}

#[test]
fn a_script_with_no_handlers_needs_uses() {
    let m = error(&riding("slayer", "settings { lives = 3 }\n"), "scoring.none");
    assert_eq!(m, "this script scores nothing. Write `uses slayer` to play Slayer's rules.");
    let m = error(&riding("oddball", "global var n: int = 0\n"), "scoring.none");
    assert!(m.ends_with("Write `uses oddball` to play Oddball's rules."), "{m}");
    // A type with no built-in game offers Slayer's.
    let m = error(&riding("party", "settings { lives = 3 }\n"), "scoring.none");
    assert!(m.contains("`uses slayer`"), "{m}");
    // Reported once the rest is clean: another error comes alone.
    let c = diagnose(&riding("slayer", "global var j: player = none\n"));
    let codes: Vec<&str> = c.errors().map(|d| d.code).collect();
    assert_eq!(codes, ["null.var"]);
    // With a handler, or with `uses`, it compiles.
    assert_eq!(compile(&riding("slayer", "on tick { }\n")).unwrap().header.uses, None);
    assert!(compile(&riding("slayer", "uses slayer\n")).is_ok());
}

#[test]
fn type_points_and_about_codes() {
    // The type is required, and one of the table's.
    let m = error("rules \"V\" 1\non tick { }\n", "decl");
    assert_eq!(m, "a script names its game type: type slayer");
    let m = error(&riding("king", "on tick { }\n"), "name.unknown");
    assert!(m.starts_with("unknown type king: a type is slayer, ctf, assault"), "{m}");
    assert!(m.ends_with("research, party, firefight, siege"), "{m}");
    let m = error(&riding("slayer", "type ctf\non tick { }\n"), "syntax");
    assert!(m.contains("the type is named twice"), "{m}");
    // Points defaults to the type's and takes 1 to 32 bytes.
    for (ty, points) in [("slayer", "Kills"), ("ctf", "Captures"), ("assault", "Detonations"), ("koth", "Time"), ("party", "Points")] {
        assert_eq!(compile(&riding(ty, "on tick { }\n")).unwrap().header.points, points, "{ty}");
    }
    let ir = compile(&riding("party", "points \"Laps\"\non tick { }\n")).unwrap();
    assert_eq!(ir.header.points, "Laps");
    let m = error(&riding("party", "points \"\"\non tick { }\n"), "limit");
    assert_eq!(m, "points is 1 to 32 bytes");
    error(&riding("party", &format!("points \"{}\"\non tick {{ }}\n", "x".repeat(33))), "limit");
    // A type files and announces; it changes no rule.
    let party = compile(&riding("party", "on kill(v, k) { score(k, 1) }\n")).unwrap();
    let slayer = compile(&riding("slayer", "on kill(v, k) { score(k, 1) }\n")).unwrap();
    assert_eq!(party.handlers, slayer.handlers);
    assert_eq!(party.header.engine(), slayer.header.engine());
    assert_ne!(party.id(), slayer.id());
    let long = "x".repeat(151);
    let m = error(&riding("slayer", &format!("uses slayer\nabout \"{long}\"\n")), "limit");
    assert!(m.starts_with("about is 151 UTF-16 units"), "{m}");
    let fits = "x".repeat(150);
    assert!(compile(&riding("slayer", &format!("uses slayer\nabout \"{fits}\"\n"))).is_ok());
}

#[test]
fn carrier_list_and_scoring_lines_are_refused_with_their_replacement() {
    let retired = |src: &str| -> Vec<String> {
        diagnose(src)
            .errors()
            .filter(|d| d.code == "header.retired")
            .map(|d| d.message.clone())
            .collect()
    };
    let m = retired("rules \"V\" 1\ntype slayer\ncarrier slayer\non tick { }\n");
    assert_eq!(m, ["carrier is gone: write `type slayer`. A script names its game type, not an engine"]);
    let m = retired("rules \"V\" 1\ntype koth\ncarrier king\non tick { }\n");
    assert!(m[0].starts_with("carrier is gone: write `type koth`"), "{m:?}");
    let m = retired("rules \"V\" 1\ntype slayer\ncarrier tetris\non tick { }\n");
    assert!(m[0].starts_with("carrier is gone: write `type` and one of slayer, ctf"), "{m:?}");
    let m = retired("rules \"V\" 1\ntype slayer\nlist party\non tick { }\n");
    assert_eq!(m, ["list is gone: write `type party`, which files the game in its row"]);
    let m = retired("rules \"V\" 1\ntype juggernaut\nscoring carrier\n");
    assert!(m[0].starts_with("scoring carrier is gone: write `uses juggernaut`"), "{m:?}");
    // With the old carrier line beside it, the uses line names that game.
    let m = retired("rules \"V\" 1\ncarrier oddball\nscoring carrier\n");
    assert!(m[1].starts_with("scoring carrier is gone: write `uses oddball`"), "{m:?}");
    // The old header alone: every line is named, and the type is missing.
    let c = diagnose("rules \"V\" 1\ncarrier slayer\nlist research\nscoring carrier\n");
    let codes: Vec<&str> = c.errors().map(|d| d.code).collect();
    assert_eq!(codes, ["decl", "header.retired", "header.retired", "header.retired"]);
}

#[test]
fn the_hash_covers_settings_marks_and_about() {
    let base = compile(SWAT).unwrap().id();
    for (from, to) in [
        ("score_limit = 50", "score_limit = 25"),
        ("    camo_pickups = false\n", ""),
        (
            "quick score_limit, starting_weapon, secondary_weapon",
            "quick starting_weapon, score_limit, secondary_weapon",
        ),
        ("50 points to win.", "Fifty points to win."),
    ] {
        let other = SWAT.replace(from, to);
        assert_ne!(other, SWAT, "{from}");
        assert_ne!(compile(&other).unwrap().id(), base, "{from}");
    }
    // The order of the block's lines is not: the IR holds them sorted.
    let swapped = SWAT.replace(
        "    team_play = true\n    score_limit = 50\n",
        "    score_limit = 50\n    team_play = true\n",
    );
    assert_ne!(swapped, SWAT);
    assert_eq!(compile(&swapped).unwrap().id(), base);
}

#[test]
fn body_codes() {
    let m = error(
        &script("player.disguise", "on round_start { set_body(team 0, \"objects/characters/elite/elite\") }\n"),
        "body.unknown",
    );
    assert!(m.contains("objects/characters/elite/elite is not a player body"), "{m}");
    assert!(m.contains("objects/characters/masterchief/masterchief_mp or objects/characters/elite/elite_mp"), "{m}");
    // Both bodies, and none, check.
    let ok = script(
        "player.disguise",
        "on round_start {\n    set_body(team 0, \"objects/characters/masterchief/masterchief_mp\")\n    set_body(team 0, \"objects/characters/elite/elite_mp\")\n    set_body(team 0, none)\n}\n",
    );
    assert!(compile(&ok).is_ok(), "{:#?}", diagnose(&ok).diagnostics);
}
