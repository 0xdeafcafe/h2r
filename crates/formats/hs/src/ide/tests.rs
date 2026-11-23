//! The HaloScript language features over an invented context: invented
//! builtins, types and scenario names. No game data.

use std::collections::BTreeMap;

use super::*;

fn b(index: u32, name: &str, returns: &str, parameters: &[&str], own: bool) -> BuiltinInfo {
    BuiltinInfo {
        index,
        name: (!name.is_empty()).then(|| name.to_owned()),
        returns: returns.into(),
        parameters: parameters.iter().map(|p| (*p).to_owned()).collect(),
        own_arguments: own,
        usage: None,
        doc: (name == "invented_print").then(|| "prints a line".to_owned()),
        from: (!name.is_empty()).then(|| "map".to_owned()),
    }
}

fn ctx() -> Context {
    let mut names = BTreeMap::new();
    names.insert("object_name".to_owned(), vec!["door".to_owned(), "crate".to_owned()]);
    names.insert("trigger_volume".to_owned(), vec!["zone".to_owned()]);
    Context {
        version: CONTEXT_VERSION,
        map: "example".into(),
        types: ["void", "boolean", "short", "string", "object_name", "unit_name", "trigger_volume", "script"]
            .map(str::to_owned)
            .to_vec(),
        script_kinds: ["startup", "dormant", "continuous", "static", "stub"].map(str::to_owned).to_vec(),
        typed_kinds: ["static", "stub"].map(str::to_owned).to_vec(),
        builtins: vec![
            b(0, "invented_begin", "void", &[], true),
            b(1, "invented_print", "void", &["string"], false),
            b(2, "invented_open", "void", &["object_name"], false),
            b(3, "invented_inside", "boolean", &["unit_name", "trigger_volume"], false),
            b(4, "invented_wake", "void", &["script"], false),
            b(5, "", "void", &["short"], false),
        ],
        external_globals: vec![NamedType {
            name: "game_speed".into(),
            type_name: "short".into(),
        }],
        object_types: vec!["object_name".into(), "unit_name".into()],
        names,
    }
}

const SRC: &str = "\
; invented scripts
(global short count 3)

(script static void helper
  (invented_open door))

(script startup main
  (invented_begin
    (helper)
    (invented_print \"hi\")
    (invented_wake helper)
    (invented_inside crate zone)
    count))
";

/// The position of the `n`th (0-based) `needle` in SRC, plus `ahead`
/// characters.
fn at(src: &str, needle: &str, n: usize, ahead: u32) -> Position {
    let off = src.match_indices(needle).nth(n).expect(needle).0;
    let p = position(src, off);
    Position::new(p.line, p.character + ahead)
}

#[test]
fn a_clean_working_copy_has_no_problems_and_reads_its_definitions() {
    let c = ctx();
    assert_eq!(diagnostics(SRC, &c), vec![]);
    let defs: Vec<_> = definitions(SRC, &c).into_iter().map(|d| (d.item, d.name, d.kind, d.line)).collect();
    assert_eq!(
        defs,
        [
            (DefItem::Global, "count".into(), "global".into(), 1),
            (DefItem::Script, "helper".into(), "static".into(), 3),
            (DefItem::Script, "main".into(), "startup".into(), 6),
        ]
    );
}

#[test]
fn problems_name_the_place_and_the_reason() {
    let c = ctx();
    let src = "(script startup main\n  (invented_begin\n    (invented_prnt \"hi\")\n    (invented_print)\n    (invented_open window)\n    (helper 1)))\n(script static void helper (invented_begin))\n(global nonsense x 1)\n(script startup main (invented_begin))\n";
    let d = diagnostics(src, &c);
    let codes: Vec<(&str, u32)> = d.iter().map(|x| (x.code.as_str(), x.range.start.line)).collect();
    assert_eq!(
        codes,
        [
            ("call.unknown", 2),
            ("call.arity", 3),
            ("name.unknown", 4),
            ("call.arity", 5),
            ("type.unknown", 7),
            ("def.duplicate", 8),
        ],
        "{d:#?}"
    );
    assert!(d[0].message.contains("did you mean invented_print?"), "{}", d[0].message);
    assert!(d[1].message.contains("takes 1 argument (invented_print <string>), not 0"), "{}", d[1].message);
    assert!(d[2].message.contains("no object in example is named window"), "{}", d[2].message);
    assert_eq!(d[2].severity, severity::WARNING);

    // Syntax: an unclosed list, a stray ), an unterminated string.
    let d = diagnostics("(global short x 1))\n(script startup s (invented_print \"open\n", &c);
    let codes: Vec<&str> = d.iter().map(|x| x.code.as_str()).collect();
    assert!(codes.contains(&"syntax.close") && codes.contains(&"syntax.open") && codes.contains(&"syntax.string"), "{codes:?}");

    // Without a context only the syntax is checked, and it says so.
    let d = diagnostics(SRC.replace("invented_print", "anything").as_str(), &Context::default());
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].code, "context.missing");
    assert_eq!(d[0].severity, severity::INFORMATION);
}

#[test]
fn completion_offers_heads_values_by_parameter_type_and_header_words() {
    let c = ctx();
    let labels = |src: &str, pos: Position| -> Vec<String> { complete(src, pos, &c).into_iter().map(|i| i.label).collect() };

    // A call's head: builtins and scripts, the unnamed one only after #.
    let src = "(script startup main\n  (inv))\n(script static void helper (invented_begin))\n";
    let got = labels(src, Position::new(1, 6));
    assert!(got.iter().all(|l| l.starts_with("inv")), "{got:?}");
    assert!(got.contains(&"invented_print".to_owned()) && !got.iter().any(|l| l.starts_with('#')));
    let src = "(script startup main\n  (\n))\n(script static void helper (invented_begin))\n";
    let got = labels(src, Position::new(1, 3));
    assert!(got.contains(&"helper".to_owned()) && got.contains(&"invented_open".to_owned()), "{got:?}");
    let src = "(script startup main\n  (#))\n";
    assert_eq!(labels(src, Position::new(1, 4)), ["#builtin:5"]);

    // An object parameter: the scenario's object names first.
    let src = "(script startup main\n  (invented_open ))\n";
    let got = labels(src, Position::new(1, 17));
    assert_eq!(&got[..2], ["crate", "door"], "{got:?}");
    assert!(!got.contains(&"zone".to_owned()));
    // A trigger volume parameter; a unit parameter takes object names.
    let src = "(script startup main\n  (invented_inside door ))\n";
    assert_eq!(labels(src, Position::new(1, 24))[0], "zone");
    // A script parameter.
    let src = "(script startup main\n  (invented_wake ))\n(script static void helper (invented_begin))\n";
    let got = labels(src, Position::new(1, 17));
    assert!(got.contains(&"helper".to_owned()) && got.contains(&"main".to_owned()), "{got:?}");

    // Header words: kinds after `script`, types after a typed kind or
    // `global`.
    assert!(labels("(script )", Position::new(0, 8)).contains(&"dormant".to_owned()));
    assert!(labels("(script static )", Position::new(0, 15)).contains(&"boolean".to_owned()));
    assert!(labels("(global )", Position::new(0, 8)).contains(&"short".to_owned()));
    assert_eq!(labels("(gl)", Position::new(0, 3)), ["global"]);

    // Nothing in a comment or a string.
    assert!(labels("; (inv", Position::new(0, 6)).is_empty());
    assert!(labels("(script startup m (invented_print \"inv\"))", Position::new(0, 37)).is_empty());
}

#[test]
fn hover_and_signature_help_read_the_builtins_and_definitions() {
    let c = ctx();
    let h = hover(SRC, at(SRC, "invented_print", 0, 2), &c).unwrap();
    assert!(h.contents.value.contains("(invented_print <string>)"), "{}", h.contents.value);
    assert!(h.contents.value.contains("prints a line"));
    assert!(h.contents.value.contains("Named by a call in this map's scripts"));
    let h = hover(SRC, at(SRC, "(helper)", 0, 2), &c).unwrap();
    assert!(h.contents.value.contains("(script static void helper)"), "{}", h.contents.value);
    assert!(h.contents.value.contains("Named 2 times"), "{}", h.contents.value);
    let h = hover(SRC, at(SRC, "door", 0, 1), &c).unwrap();
    assert_eq!(h.contents.value, "`door`: an object name in example.");
    let h = hover(SRC, at(SRC, "zone", 0, 1), &c).unwrap();
    assert_eq!(h.contents.value, "`zone`: a trigger volume in example.");
    assert!(hover(SRC, at(SRC, "count))", 0, 1), &c).unwrap().contents.value.contains("(global short count)"));

    let src = "(script startup main\n  (invented_inside door ))\n";
    let s = signature_help(src, Position::new(1, 24), &c).unwrap();
    assert_eq!(s.signature.label, "(invented_inside <unit_name> <trigger_volume>)");
    assert_eq!(s.active_parameter, 1);
    assert_eq!(s.signature.parameters, [[17, 28], [29, 45]]);
    assert!(signature_help(src, Position::new(1, 5), &c).is_none(), "on the name itself");
}

#[test]
fn definition_references_highlights_and_rename_follow_scripts_and_globals() {
    let c = ctx();
    let call = at(SRC, "(helper)", 0, 2);
    let decl = Range {
        start: Position::new(3, 20),
        end: Position::new(3, 26),
    };
    assert_eq!(definition(SRC, call, &c), [decl]);
    let refs = references(SRC, call, &c, true);
    assert_eq!(refs.len(), 3, "{refs:?}");
    assert_eq!(refs[0], decl);
    assert_eq!(references(SRC, call, &c, false).len(), 2);
    let marks = highlights(SRC, call, &c);
    assert_eq!(marks[0].kind, highlight_kind::WRITE);
    assert!(marks[1..].iter().all(|m| m.kind == highlight_kind::READ));
    // A global and a scenario name.
    assert_eq!(references(SRC, at(SRC, "count))", 0, 0), &c, true).len(), 2);
    assert_eq!(references(SRC, at(SRC, "door", 0, 0), &c, true).len(), 1);

    let edits = rename(SRC, call, &c, "helper_two").unwrap();
    assert_eq!(edits.len(), 3);
    assert!(edits.iter().all(|e| e.new_text == "helper_two"));
    assert!(rename(SRC, call, &c, "main").unwrap_err().contains("already"));
    assert!(rename(SRC, call, &c, "two words").is_err());
    assert!(prepare_rename(SRC, at(SRC, "invented_print", 0, 1), &c).unwrap_err().contains("builtin"));
    assert_eq!(prepare_rename(SRC, call, &c).unwrap().placeholder, "helper");
}

#[test]
fn the_outline_folding_and_tokens_cover_every_definition() {
    let c = ctx();
    let s = symbols(SRC, &c);
    assert_eq!(s.iter().map(|x| (x.name.as_str(), x.kind)).collect::<Vec<_>>(), [("count", 13), ("helper", 12), ("main", 12)]);
    assert_eq!(s[1].detail, "static void");
    let f = folding_ranges("(script startup main\n  (invented_begin\n    1))\n;* a\nb *;\n");
    assert_eq!(
        f,
        [
            FoldingRange { start_line: 0, end_line: 2, kind: None },
            FoldingRange { start_line: 1, end_line: 2, kind: None },
            FoldingRange { start_line: 3, end_line: 4, kind: Some("comment") },
        ]
    );
    let t = semantic_tokens(SRC, &c);
    let kind_of = |line: u32, start: u32| t.iter().find(|k| k.line == line && k.start == start).map(|k| k.kind);
    assert_eq!(kind_of(0, 0), Some(TokenKind::Comment));
    assert_eq!(kind_of(1, 1), Some(TokenKind::Keyword));
    assert_eq!(kind_of(1, 8), Some(TokenKind::Type));
    assert_eq!(kind_of(1, 14), Some(TokenKind::Variable));
    assert_eq!(kind_of(1, 20), Some(TokenKind::Number));
    assert_eq!(kind_of(4, 3), Some(TokenKind::Function));
    assert_eq!(kind_of(4, 17), Some(TokenKind::EnumMember));
    assert_eq!(kind_of(7, 3), Some(TokenKind::Macro));
    assert_eq!(kind_of(8, 5), Some(TokenKind::Method));
    assert_eq!(kind_of(9, 20), Some(TokenKind::String));
    // In order, one line each.
    assert!(t.windows(2).all(|w| (w[0].line, w[0].start) < (w[1].line, w[1].start)));
    let block = semantic_tokens(";* one\ntwo *;", &c);
    assert_eq!(block.iter().map(|k| (k.line, k.len)).collect::<Vec<_>>(), [(0, 6), (1, 6)]);
}

#[test]
fn the_formatter_indents_by_depth_and_keeps_comments_and_line_breaks() {
    let messy = "\n\n(script startup main   \n(invented_begin\n      (helper) ; why\n\n\n  (invented_print \"a  b\")))\n;* keep\n     this *;\n";
    let want = "(script startup main\n  (invented_begin\n    (helper) ; why\n\n    (invented_print \"a  b\")))\n;* keep\n     this *;\n";
    assert_eq!(format(messy).unwrap(), want);
    assert_eq!(format(want).unwrap(), want, "a second pass changes nothing");
    assert!(format("(script startup main (x)").unwrap_err().contains("never closed"));
    // The decompiler's own layout is already formatted.
    let decompiled = "(script startup main\n  (invented_print \"hi\" count))\n";
    assert_eq!(format(decompiled).unwrap(), decompiled);
}

#[test]
fn a_misspelt_call_has_a_quick_fix_and_names_are_found_for_studio() {
    let c = ctx();
    let src = "(script startup main\n  (invented_prnt \"hi\"))\n";
    let a = actions(src, Range { start: Position::new(1, 4), end: Position::new(1, 4) }, &c);
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].title, "Change to `invented_print`");
    assert_eq!(a[0].edits[0].range.start, Position::new(1, 3));

    assert_eq!(name_at(SRC, at(SRC, "(helper)", 0, 2), &c), Some(("script", "helper".into())));
    assert_eq!(name_at(SRC, at(SRC, "count))", 0, 1), &c), Some(("global", "count".into())));
    assert_eq!(name_at(SRC, at(SRC, "door", 0, 1), &c), Some(("object", "door".into())));
    assert_eq!(name_at(SRC, at(SRC, "zone", 0, 1), &c), Some(("volume", "zone".into())));
    // Off a name: the definition around it.
    assert_eq!(name_at(SRC, at(SRC, "\"hi\"", 0, 1), &c), Some(("script", "main".into())));
    assert_eq!(name_at(SRC, Position::new(0, 3), &c), None, "a comment outside any definition");
}

#[test]
fn layout_and_comments_do_not_change_a_normalised_definition() {
    assert_eq!(
        normalise("(script startup main\n  ; note\n  (invented_print   \"a  b\"  ) )"),
        "(script startup main (invented_print \"a  b\"))"
    );
    assert_ne!(normalise("(a b)"), normalise("(a c)"));
}

#[test]
fn the_context_file_round_trips_and_refuses_another_version() {
    let c = ctx();
    assert_eq!(Context::parse(&c.to_json()).unwrap(), c);
    let mut old = c.clone();
    old.version = 99;
    assert!(Context::parse(&old.to_json()).unwrap_err().contains("format 99"));
    assert!(Context::parse("{}").is_err(), "no version");
    assert_eq!(c.names_for("unit_name").unwrap(), ["door", "crate"]);
    assert_eq!(c.builtins[5].call_form(), "(#builtin:5 <short>)");
}

#[test]
fn positions_count_utf16_units() {
    let src = "; \u{1F600}\n(global short x 1)";
    assert_eq!(position(src, src.find('x').unwrap()), Position::new(1, 14));
    assert_eq!(offset(src, Position::new(0, 4)), src.find('\n').unwrap());
    assert_eq!(offset(src, Position::new(9, 0)), src.len());
}
