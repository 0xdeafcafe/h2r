//! The compiler on invented tables: every construct round-trips through
//! the decompiler, an edit changes only its own nodes, and each class of
//! problem is a message with its place.

use super::*;
use crate::Expression;
use crate::fixture::{Builtin, Exe, TYPES, Tables};

/// Invented types at the indices the compiler and casts read by number.
fn exe() -> Exe {
    let mut types: Vec<String> = (0..TYPES).map(|i| format!("t{i}")).collect();
    for (i, n) in [
        (2, "function_name"),
        (3, "passthrough"),
        (4, "void"),
        (5, "boolean"),
        (6, "real"),
        (7, "short"),
        (8, "long"),
        (9, "string"),
        (10, "script"),
        (11, "invented_speed"),
        (50, "object"),
        (56, "object_name"),
    ] {
        types[i] = n.to_owned();
    }
    let b = |returns, flags, parameters: &[u16]| Builtin {
        returns,
        flags,
        parameters: parameters.to_vec(),
        usage: None,
    };
    Exe {
        types,
        builtins: vec![
            b(4, 0, &[]),      // 0: unused
            b(4, 0, &[9, 7]),  // 1: invented_print
            b(3, 2, &[]),      // 2: begin (reads its own arguments)
            b(3, 2, &[]),      // 3: invented_if (reads its own arguments)
            b(6, 0, &[6, 6]),  // 4: invented_add
            b(4, 0, &[50, 6]), // 5: invented_move
            b(5, 0, &[]),      // 6: invented_flag
            b(8, 0, &[8]),     // 7: invented_long
            b(4, 0, &[10]),    // 8: invented_wake
            b(4, 0, &[11]),    // 9: invented_speed_set
        ],
        external_type: 6,
        ..Exe::default()
    }
}

/// Two globals and two scripts using every construct the decompiler
/// prints: calls of fixed and self-reading builtins, a script call,
/// scenario and game globals, casts, and literals of every form.
fn tables() -> Tables {
    let mut t = Tables::new();
    let four = t.value(7, 4);
    t.global_definition("count", 7, four);
    let half = t.value(6, 1.5f32.to_bits());
    let count = t.global(0, "count", 7);
    t.retype(count, 6);
    let add = t.call(4, "invented_add", 6, &[half, count]);
    t.global_definition("ratio", 6, add);

    let hi = t.literal(9, "hi \"there\"\n", 0);
    let minus3 = t.value(7, 0xfffd);
    let print1 = t.call(1, "invented_print", 4, &[hi, minus3]);
    let flag = t.call(6, "invented_flag", 5, &[]);
    let other = t.script_call(1, "other", 5, &[]);
    let x = t.literal(9, "x", 0);
    let seven = t.value(7, 7);
    let print2 = t.call(1, "invented_print", 4, &[x, seven]);
    let iff = t.call(3, "invented_if", 3, &[flag, other, print2]);
    let door = t.literal(56, "door_a", 0x1234);
    t.retype(door, 50);
    let speed = t.global(0x8003, "ext_speed", 6);
    let move1 = t.call(5, "invented_move", 4, &[door, speed]);
    let big = t.value(8, 70000);
    let long = t.call(7, "invented_long", 8, &[big]);
    let script_ref = t.literal(10, "other", 1);
    let wake = t.call(8, "invented_wake", 4, &[script_ref]);
    let fast = t.literal(11, "fast", 2);
    let set = t.call(9, "invented_speed_set", 4, &[fast]);
    let door_b = t.literal(56, "door b", 0x1235);
    t.retype(door_b, 50);
    let zero = t.value(6, (-0.0f32).to_bits());
    let move2 = t.call(5, "invented_move", 4, &[door_b, zero]);
    let none = t.value(4, 0);
    let yes = t.value(5, 1);
    t.retype(yes, 4);
    let begin = t.call(
        2,
        "begin",
        4,
        &[print1, iff, move1, long, wake, set, move2, none, yes],
    );
    t.script("main", 0, 4, begin);
    let f = t.call(6, "invented_flag", 5, &[]);
    t.script("other", 3, 5, f);
    t
}

fn setup() -> (Scenario, Catalog) {
    let s = tables().scenario();
    let mut catalog = exe().catalog();
    catalog.observe(&s).unwrap();
    (s, catalog)
}

fn ok(s: &Scenario, c: &Catalog, src: &str) -> Compiled {
    compile(s, c, src, &Options::default()).unwrap_or_else(|e| panic!("{src}\n{e:#?}"))
}

/// Each message's code, line and column.
fn problems(s: &Scenario, c: &Catalog, src: &str) -> Vec<(&'static str, u32, u32)> {
    match compile(s, c, src, &Options::default()) {
        Ok(_) => panic!("{src} compiled"),
        Err(e) => e.iter().map(|m| (m.code, m.line, m.column)).collect(),
    }
}

fn codes(s: &Scenario, c: &Catalog, src: &str) -> Vec<&'static str> {
    problems(s, c, src).into_iter().map(|p| p.0).collect()
}

#[test]
fn every_construct_round_trips_through_the_decompiler() {
    let (s, catalog) = setup();
    let source: String = s
        .decompile_each(&catalog)
        .unwrap()
        .into_iter()
        .map(|e| e.source)
        .collect::<Vec<_>>()
        .join("\n");
    let out = ok(&s, &catalog, &source);
    let c = &out.scenario;
    assert_eq!(
        out.items,
        [
            Item::Global(0),
            Item::Global(1),
            Item::Script(0),
            Item::Script(1)
        ]
    );
    assert_eq!(c.scripts().len(), s.scripts().len());
    for (a, b) in s.scripts().iter().zip(c.scripts()) {
        assert_eq!(
            (a.name_bytes, a.kind, a.return_type),
            (b.name_bytes, b.kind, b.return_type)
        );
        assert_eq!(first_difference(&s, a.root, c, b.root).unwrap(), None, "{}", a.name().unwrap());
    }
    for (a, b) in s.globals().iter().zip(c.globals()) {
        assert_eq!((a.name_bytes, a.value_type), (b.name_bytes, b.value_type));
        assert_eq!(first_difference(&s, a.root, c, b.root).unwrap(), None, "{}", a.name().unwrap());
    }
    assert_eq!(
        c.decompile(&catalog, None).unwrap().source,
        s.decompile(&catalog, None).unwrap().source
    );
    // Every old node is freed and every new one takes a freed slot.
    assert_eq!((out.freed, out.reused, out.grown), (s.nodes().len(), s.nodes().len(), 0));
    assert_eq!(c.nodes().len(), s.nodes().len());
    assert_eq!(out.new_strings, 0);
}

#[test]
fn a_changed_script_changes_only_its_own_nodes() {
    let (s, catalog) = setup();
    let out = ok(
        &s,
        &catalog,
        "(script static boolean other\n  (invented_if (invented_flag) true false))\n\
         (script dormant later (invented_print \"brand new\" 1))",
    );
    let c = &out.scenario;
    assert_eq!(out.items, [Item::Script(1), Item::Script(2)]);
    // `other` was one call and its name: two slots, reused first.
    assert_eq!(out.freed, 2);
    assert_eq!(out.nodes, 6 + 4);
    assert_eq!((out.reused, out.grown), (2, 8));
    assert_eq!(out.new_strings, "brand new".len() + 1);
    let old_other = structural(&s, &[s.scripts()[1].root()]);
    for (i, n) in s.nodes().iter().enumerate() {
        if !old_other.contains(&i) {
            assert_eq!(c.nodes()[i], *n, "slot {i} changed");
        }
    }
    assert_eq!(&c.strings()[..s.strings().len()], s.strings());
    assert_eq!(c.scripts()[0], s.scripts()[0]);
    let later = &c.scripts()[2];
    assert_eq!((later.name().unwrap(), later.kind(), later.return_type()), ("later", 1, 4));
    let other = c.decompile(&catalog, Some("other")).unwrap().source;
    assert_eq!(
        other,
        "(script static boolean other\n  (invented_if\n    (invented_flag) true false))\n\n"
    );
    // The `if` takes the types the map's own `if` gives each position.
    let Expression::Call { children, result_type, .. } = c.expression(c.scripts()[1].root()).unwrap() else {
        panic!("a call");
    };
    assert_eq!(result_type, 5);
    let types: Vec<(u16, u16)> = children
        .map(|d| {
            let n = c.node(d.unwrap()).unwrap();
            (n.opcode(), n.value_type())
        })
        .collect();
    assert_eq!(types, [(6, 5), (5, 5), (5, 4)]);
}

#[test]
fn several_body_expressions_are_joined_by_begin() {
    let (s, catalog) = setup();
    let out = ok(
        &s,
        &catalog,
        "(script startup two (invented_print \"a\" 1) (invented_print \"b\" 2))",
    );
    let c = &out.scenario;
    let root = c.node(c.scripts()[2].root()).unwrap();
    assert_eq!((root.opcode(), root.value_type(), root.flags()), (2, 4, CALL));
}

#[test]
fn literals_take_their_form_and_names_resolve_through_the_caller() {
    let (s, catalog) = setup();
    let resolve: Resolve<'_> = &|t: u16, name: &str, token: &[u8]| {
        (t == 50 && name == "object" && token == b"crate_1").then_some(7u32)
    };
    let options = Options {
        resolve: Some(resolve),
        ..Options::default()
    };
    let out = compile(
        &s,
        &catalog,
        "(script startup lits (begin (invented_move crate_1 #real-bits:7fc00001) (invented_long -2) (invented_wake main) (invented_print bare 0)))",
        &options,
    )
    .unwrap();
    let c = &out.scenario;
    let atoms: Vec<(u16, u16, u32)> = c
        .nodes()
        .iter()
        .filter(|n| n.live() && n.atom() && n.value_type() != FUNCTION_NAME && !n.global())
        .map(|n| (n.opcode(), n.value_type(), n.data()))
        .collect();
    assert!(atoms.contains(&(50, 50, 7)), "{atoms:?}");
    assert!(atoms.contains(&(6, 6, 0x7fc0_0001)), "{atoms:?}");
    assert!(atoms.contains(&(8, 8, (-2i32) as u32)), "{atoms:?}");
    // A script named where a script is wanted: its index, with the upper
    // half the map's script literals agree on.
    assert!(atoms.contains(&(10, 10, 0)), "{atoms:?}");
    // A bare word where a string is wanted is that string.
    let bare = c.nodes().iter().find(|n| n.live() && n.opcode() == 9 && c.string_bytes(n.string_offset()).unwrap() == b"bare").unwrap();
    assert_eq!(bare.data(), bare.string_offset());
}

#[test]
fn unnamed_builtins_are_called_by_index() {
    let mut t = Tables::new();
    let f = t.call(6, "invented_flag", 5, &[]);
    t.script("only", 3, 5, f);
    let s = t.scenario();
    let catalog = exe().catalog();
    // Unobserved: only the scenario's own calls name builtins.
    let out = ok(&s, &catalog, "(script static boolean b (#builtin:6))\n(script static void v (#builtin:7 5))");
    let c = &out.scenario;
    let head = |i: usize| {
        let call = c.node(c.scripts()[i].root()).unwrap();
        c.string(c.node(Datum(call.data())).unwrap().string_offset()).unwrap().to_owned()
    };
    // Index 6 is spelled as the map spells it; 7 has no name.
    assert_eq!(head(1), "invented_flag");
    assert_eq!(head(2), "#builtin:7");
    assert_eq!(
        codes(&s, &catalog, "(script startup e (invented_flag) (invented_flag))"),
        ["body.many"]
    );
    assert_eq!(codes(&s, &catalog, "(script startup e (#builtin:99999))"), ["call.unknown"]);
}

#[test]
fn each_problem_has_its_code_and_place() {
    let (s, c) = setup();
    let one = |src: &str| codes(&s, &c, src);
    assert_eq!(one("(script startup a (invented_flag)"), ["syntax.open"]);
    assert_eq!(one("(script startup a (invented_flag)))"), ["syntax.close"]);
    assert_eq!(one("(script startup a (invented_print \"x\n 1))"), ["syntax.string"]);
    assert_eq!(one("(script startup a (invented_print \"\\q\" 1))"), ["syntax.escape"]);
    assert_eq!(one("(script startup a (invented_print \"\\x00\" 1))"), ["syntax.escape"]);
    assert_eq!(one("(defun a (invented_flag))"), ["def.head"]);
    assert_eq!(one("stray"), ["def.head"]);
    assert_eq!(one("(script sometimes a (invented_flag))"), ["def.kind"]);
    assert_eq!(one("(global quaternion q 1)"), ["def.type"]);
    assert_eq!(one("(script startup a_name_of_thirty_two_bytes_long_ (invented_flag))"), ["def.name"]);
    assert_eq!(one("(script startup a)"), ["def.body"]);
    assert_eq!(one("(global short g 1 2)"), ["def.body"]);
    assert_eq!(
        one("(script startup a (invented_flag))\n(script startup a (invented_flag))"),
        ["def.duplicate"]
    );
    assert_eq!(one("(script startup a ())"), ["call.empty"]);
    assert_eq!(one("(script startup a ((invented_flag)))"), ["call.head"]);
    assert_eq!(one("(script startup a (no_such_thing))"), ["call.unknown"]);
    assert_eq!(one("(script startup a (invented_print \"x\"))"), ["call.arity"]);
    assert_eq!(one("(script startup a (other 1))"), ["call.script_arguments"]);
    assert_eq!(one("(script startup a (invented_print \"x\" (invented_flag)))"), ["type.cast"]);
    assert_eq!(one("(script startup a (invented_print \"x\" 70000))"), ["value.range"]);
    assert_eq!(one("(script startup a (invented_long 1.5.2))"), ["value.number"]);
    assert_eq!(one("(script startup a (invented_print \"x\" word))"), ["value.type"]);
    assert_eq!(one("(script startup a (invented_move door_z 1.0))"), ["name.unresolved"]);
    assert_eq!(
        one("(script startup a (begin (invented_flag) mystery (invented_flag)))"),
        ["name.unknown"]
    );
    let deep = format!("(script startup a {}(invented_flag){})", "(begin ".repeat(MAX_DEPTH + 2), ")".repeat(MAX_DEPTH + 2));
    assert_eq!(one(&deep), ["call.depth"]);
    // Several problems are all reported, in source order.
    assert_eq!(
        one("(script startup a (no_such_thing))\n(script startup b (invented_print \"x\"))"),
        ["call.unknown", "call.arity"]
    );
}

#[test]
fn a_message_names_its_line_and_column() {
    let (s, c) = setup();
    let e = compile(&s, &c, "(script startup a\n  (no_such_thing))", &Options::default()).unwrap_err();
    assert_eq!((e[0].line, e[0].column), (2, 4));
    assert_eq!(e[0].text, "nothing is named `no_such_thing`: no script of this map and no named builtin has that name");
    assert_eq!(e[0].to_string(), format!("2:4: {}", e[0].text));
    assert_eq!(
        problems(&s, &c, "(script startup a\n  (invented_print \"x\"\n    (invented_flag)))"),
        [("type.cast", 3, 5)]
    );
}

#[test]
fn a_compile_past_the_node_limit_is_refused_with_both_counts() {
    let (s, c) = setup();
    let options = Options {
        node_limit: Some(s.nodes().len()),
        ..Options::default()
    };
    let e = compile(&s, &c, "(script startup more (invented_flag))", &options).unwrap_err();
    assert_eq!(e[0].code, "nodes.limit");
    let need = s.nodes().len() + 2;
    assert!(e[0].text.contains(&need.to_string()) && e[0].text.contains(&s.nodes().len().to_string()), "{}", e[0].text);
    // Replacing a script in place needs no new slots.
    compile(&s, &c, "(script static boolean other (invented_flag))", &options).unwrap();
}

#[test]
fn a_shared_node_stays_with_the_definition_that_keeps_it() {
    let (s, c) = setup();
    // Point `ratio`'s value at `count`'s literal: replacing `count` must
    // not free a slot `ratio` still reaches.
    let mut s = s;
    let shared = s.globals[0].root;
    s.globals[1].root = shared;
    let out = ok(&s, &c, "(global short count 9)");
    assert_eq!(out.freed, 0);
    assert_eq!(out.scenario.node(shared).unwrap(), s.node(shared).unwrap());
}
