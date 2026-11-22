//! The checker: one case per diagnostic code, every error reported (up to
//! fifty), declared labels in the label table, handlers by label, `is`
//! and `T?` narrowing, the weapon tag group, and time, constants, `match`
//! and value functions. See `docs/game/rules-language-types.md`,
//! "Stronger typing" and "Diagnostics".

use rules::fake::FakeGame;
use rules::ir::{LabelKind, TagGroup, label_flags};
use rules::{Event, Input, Severity, Vm, builtin, compile, diagnose};

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
        "const.range",
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
    let int = compile(&script("", "option hold: int 5..60 = 20 \"Capture time\"\n")).unwrap();
    let time = compile(&script("", "option hold: time 5s..60s = 20s \"Capture time\"\n")).unwrap();
    let (a, b) = (&int.options[0], &time.options[0]);
    assert_eq!((a.ty, a.lo, a.hi, a.default, a.bits()), (b.ty, b.lo, b.hi, b.default, b.bits()));
    assert_eq!(int.default_options(), time.default_options());
    assert_eq!(int.strings[a.label as usize], "Capture time");
    assert_eq!(time.strings[b.label as usize], "Capture time (seconds)");
    // With no label, the name carries the unit.
    let bare = compile(&script("", "option hold: time 5s..60s = 20s\n")).unwrap();
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
fn variant_engine_is_the_carrier_enum() {
    let src = script(
        "",
        "global var out: int = 0
on tick {
    if variant.engine == carrier.slayer { out = 1 }
    match variant.engine {
        carrier.ctf, carrier.assault { out = 2 }
        else { }
    }
}
",
    );
    assert!(compile(&src).is_ok(), "{:#?}", diagnose(&src).diagnostics);
    let m = error(&src.replace("carrier.slayer", "2"), "type.mismatch");
    assert!(m.contains("write carrier.slayer"), "{m}");
    let m = error(&src.replace("else { }", ""), "match.missing");
    assert!(m.contains("carrier.slayer"), "{m}");
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
    assert!(builtin::SIEGE.contains("const last_phase"));
    assert!(builtin::ODDBALL.contains("player var held: time"));
    assert!(builtin::VIP.contains("match victim.role"));
    assert!(builtin::KING.contains("fn rival(a: player, b: player) -> bool ="));
    assert!(builtin::CTF.contains("carrier.assault"));
    // A time option's label carries its unit in the blob.
    let siege = compile(builtin::SIEGE).unwrap();
    let labels: Vec<&str> = siege.options.iter().map(|o| siege.strings[o.label as usize].as_str()).collect();
    assert_eq!(labels, ["Phase time (seconds)", "Capture time (seconds)", "Respawn wave (seconds)"]);
}
