use super::model::LineIndex;
use super::scan::{self, K};
use super::*;

const SRC: &str = "rules \"T\" 1
carrier slayer
teams 2
requires player.team

enum Role { none, vip }
option points: int 1..5 = 2 \"Points\"
player var role: Role = Role.none
global var clock: timer
table bonus: (r: Role, n: int) = [(Role.vip, 3)]

fn reward(p: player) {
    score(p, points)
}

on kill(victim, killer) {
    let k = killer
    if k != none and k.role == Role.vip {
        reward(k)
    }
    for each players q where q.team == victim.team {
        set_team(q, team 1)
    }
}

on timer(clock) {
    end_round(team 0)
}
";

fn p(line: u32, character: u32) -> Position {
    Position::new(line, character)
}

fn hover_text(src: &str, line: u32, character: u32) -> String {
    hover(src, p(line, character))
        .unwrap_or_else(|| panic!("no hover at {line}:{character}"))
        .markdown
}

fn labels(items: &[CompletionItem]) -> Vec<&str> {
    items.iter().map(|i| i.label.as_str()).collect()
}

/// Every built-in and example script.
fn scripts() -> Vec<(&'static str, &'static str)> {
    let mut out: Vec<(&str, &str)> = crate::builtin::ALL.iter().map(|b| (b.name, b.source)).collect();
    out.extend(crate::builtin::examples().iter().copied());
    out
}

#[test]
fn positions_count_utf16_units() {
    let li = LineIndex::new("a\u{1F600}b\nc");
    assert_eq!(li.position(5), p(0, 3));
    assert_eq!(li.offset(p(0, 3)), 5);
    assert_eq!(li.position(7), p(1, 0));
    assert_eq!(li.offset(p(1, 9)), 8);
    assert_eq!(li.offset(p(5, 0)), 8);
}

#[test]
fn a_clean_script_has_only_its_unhostable_note() {
    let d = diagnostics(SRC);
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].severity, severity::INFORMATION);
    assert_eq!(d[0].code, "unhostable");
    assert_eq!(d[0].range.start, p(21, 8));
    assert!(d[0].message.contains("set_team"));
}

#[test]
fn diagnostics_report_every_syntax_error_unused_names_and_compile_errors() {
    let src = "rules \"T\" 1\ncarrier slayer\non tick {\n    score(\n}\non spawn(p) { let spare = 1 }\nfn idle() { }\n";
    let d = diagnostics(src);
    assert!(
        d.iter()
            .any(|x| x.severity == severity::ERROR && x.code == "syntax" && x.range.start.line == 4),
        "{d:?}"
    );
    assert!(d.iter().any(|x| x.severity == severity::WARNING && x.message.contains("`spare`")));
    assert!(d.iter().any(|x| x.severity == severity::WARNING && x.message.contains("fn `idle`")));

    let src = "rules \"T\" 1\ncarrier slayer\non tick { score(nobody, 1) }\n";
    let d = diagnostics(src);
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].code, "compile");
    assert_eq!(d[0].range.start, p(2, 16));
    assert_eq!(d[0].range.end, p(2, 22));

    // A lexer error does not hide the parser's.
    let d = diagnostics("rules \"T\" 1\ncarrier slayer\non tick { @ }\non spawn(p) { score(p, +) }\n");
    let lines: Vec<u32> = d.iter().map(|x| x.range.start.line).collect();
    assert!(lines.contains(&2) && lines.contains(&3), "{d:?}");
}

#[test]
fn hover_names_types_entries_events_and_fields() {
    let score = hover_text(SRC, 12, 5);
    assert!(score.contains("score(player, int)"), "{score}");
    assert!(score.contains("Status: named"), "{score}");
    let set_team = hover_text(SRC, 21, 9);
    assert!(set_team.contains("requires player.team"), "{set_team}");
    assert!(set_team.contains("moves players between teams"), "{set_team}");
    assert!(set_team.contains("untraced"), "{set_team}");
    assert!(hover_text(SRC, 18, 15).contains("let k: player"));
    assert!(hover_text(SRC, 20, 40).contains("(argument) victim: player"));
    let vip = hover_text(SRC, 17, 37);
    assert!(vip.contains("Role.vip") && vip.contains("enum `Role`"), "{vip}");
    assert!(hover_text(SRC, 17, 24).contains("A variable of each player"));
    assert!(hover_text(SRC, 15, 4).contains("on kill(victim: player, killer: player)"));
    assert!(hover_text(SRC, 20, 32).contains("team_of"));
    assert!(hover_text(SRC, 3, 12).contains("moves players between teams"));
    assert!(hover_text(SRC, 1, 9).contains("engine index 2"));
    assert!(hover_text(SRC, 6, 16).contains("32-bit"));
    assert!(hover_text(SRC, 16, 5).contains("A name for a value"));
    assert!(hover_text(SRC, 11, 5).contains("fn reward(p: player)"));
    let h = hover(SRC, p(12, 5)).unwrap();
    assert_eq!(h.range, Range { start: p(12, 4), end: p(12, 9) });
    assert!(hover(SRC, p(4, 0)).is_none());
}

#[test]
fn definitions_and_references_resolve_through_scopes() {
    let at = |l: &[Location]| -> Vec<(u32, u32)> {
        l.iter().map(|x| (x.range.start.line, x.range.start.character)).collect()
    };
    assert_eq!(at(&definition(SRC, p(18, 15))), vec![(16, 8)]);
    assert_eq!(at(&definition(SRC, p(25, 10))), vec![(8, 11)]);
    assert_eq!(at(&definition(SRC, p(18, 9))), vec![(11, 3)]);
    assert_eq!(at(&definition(SRC, p(12, 15))), vec![(6, 7)]);
    // The end of a name counts as on it.
    assert_eq!(at(&definition(SRC, p(18, 16))), vec![(16, 8)]);
    assert_eq!(at(&references(SRC, p(17, 24))), vec![(7, 11), (17, 23)]);
    assert_eq!(
        at(&references(SRC, p(5, 19))),
        vec![(5, 18), (9, 40), (17, 36)]
    );
    assert!(definition(SRC, p(12, 5)).is_empty());
}

#[test]
fn signature_help_tracks_the_argument() {
    let s = signature_help(SRC, p(12, 13)).unwrap();
    assert_eq!(s.signatures[0].label, "score(player, int)");
    assert_eq!(s.active_parameter, 1);
    let params: Vec<&str> = s.signatures[0].parameters.iter().map(|x| x.label.as_str()).collect();
    assert_eq!(params, ["player", "int"]);
    assert_eq!(s.signatures[0].parameters[1].offsets, [14, 17]);
    let s = signature_help(SRC, p(18, 15)).unwrap();
    assert_eq!(s.signatures[0].label, "reward(p: player)");
    assert_eq!(s.active_parameter, 0);
    let s = signature_help(SRC, p(15, 9)).unwrap();
    assert_eq!(s.signatures[0].label, "kill(victim: player, killer: player)");
    assert!(signature_help(SRC, p(16, 10)).is_none());
}

#[test]
fn completion_follows_the_context() {
    let src = "rules \"T\" 1\ncarrier slayer\nplayer var role: int = 0\nenum Role { none, vip }\non kill(victim, killer) {\n    killer.\n}\n";
    let items = complete(src, p(5, 11));
    let l = labels(&items);
    assert!(l.contains(&"role") && l.contains(&"team") && l.contains(&"score"), "{l:?}");
    assert!(!l.contains(&"let"));

    let body = "rules \"T\" 1\ncarrier slayer\non kill(victim, killer) {\n    let x = variant.score_limit\n    \n}\n";
    assert!(labels(&complete(body, p(3, 20))).contains(&"score_limit"));
    let items = complete(body, p(4, 4));
    let l = labels(&items);
    assert!(l.contains(&"victim") && l.contains(&"let") && l.contains(&"count"), "{l:?}");
    let score = items.iter().find(|i| i.label == "score").unwrap();
    assert_eq!(score.kind, kind::FUNCTION);
    assert_eq!(score.insert_text_format, SNIPPET);
    assert_eq!(score.insert_text, "score(${1:player}, ${2:int})");
    assert!(score.documentation.value.contains("Status: named"));

    let top = "rules \"T\" 1\n\n";
    let l = labels(&complete(top, p(1, 0)));
    assert!(l.contains(&"option") && l.contains(&"global var") && l.contains(&"on kill"), "{l:?}");

    let on = "rules \"T\" 1\non ";
    let items = complete(on, p(1, 3));
    let kill = items.iter().find(|i| i.label == "kill").unwrap();
    assert_eq!(kill.insert_text, "kill(${1:victim}, ${2:killer}) {\n\t$0\n}");
    assert_eq!(kill.kind, kind::EVENT);

    assert!(labels(&complete("rules \"T\" 1\ncarrier ", p(1, 8))).contains(&"slayer"));
    assert!(labels(&complete("rules \"T\" 1\nrequires ", p(1, 9))).contains(&"player.team"));
    let forge = labels(&complete("rules \"T\" 1\nrequires forge.", p(1, 15)));
    assert!(forge.contains(&"place") && !forge.contains(&"player.team"), "{forge:?}");
    let types = labels(&complete("rules \"T\" 1\nenum Role { none, vip }\nplayer var role: \n", p(2, 17)));
    assert!(types.contains(&"int") && types.contains(&"Role"), "{types:?}");
    let members = "rules \"T\" 1\nenum Role { none, vip }\non tick {\n    let r = Role.\n}\n";
    assert_eq!(labels(&complete(members, p(3, 17))), ["none", "vip"]);
    // Nothing in a comment.
    assert!(complete("rules \"T\" 1\n// on \n", p(1, 6)).is_empty());
}

#[test]
fn semantic_tokens_classify_names_by_what_they_are() {
    let tokens = semantic_tokens(SRC);
    let at = |line: u32, start: u32| -> TokenKind {
        tokens
            .iter()
            .find(|t| t.line == line && t.start == start)
            .unwrap_or_else(|| panic!("no token at {line}:{start}"))
            .kind
    };
    assert_eq!(at(0, 0), TokenKind::Keyword);
    assert_eq!(at(0, 6), TokenKind::String);
    assert_eq!(at(0, 10), TokenKind::Number);
    assert_eq!(at(3, 9), TokenKind::Capability);
    assert_eq!(tokens.iter().find(|t| t.line == 3 && t.start == 9).unwrap().len, 11);
    assert_eq!(at(5, 5), TokenKind::Enum);
    assert_eq!(at(5, 12), TokenKind::EnumMember);
    assert_eq!(at(6, 7), TokenKind::Variable);
    assert_eq!(at(6, 15), TokenKind::Type);
    assert_eq!(at(11, 3), TokenKind::Function);
    assert_eq!(at(11, 10), TokenKind::Parameter);
    assert_eq!(at(12, 4), TokenKind::Api);
    assert_eq!(at(15, 3), TokenKind::Event);
    assert_eq!(at(15, 8), TokenKind::Parameter);
    assert_eq!(at(16, 8), TokenKind::Variable);
    assert_eq!(at(17, 9), TokenKind::Operator);
    assert_eq!(at(17, 36), TokenKind::EnumMember);
    let labelled = semantic_tokens("rules \"T\" 1\ncarrier slayer\non tick { for each objects labelled \"x\" o { } }\n");
    assert!(labelled.iter().any(|t| t.line == 2 && t.start == 36 && t.kind == TokenKind::Label));
    assert_eq!(LEGEND[TokenKind::Capability.index() as usize], "capability");
}

#[test]
fn symbols_outline_the_declarations() {
    let s = symbols(SRC);
    let names: Vec<&str> = s.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(
        names,
        ["T", "Role", "points", "role", "clock", "bonus", "reward", "on kill", "on timer"]
    );
    let role = &s[1];
    assert_eq!(role.kind, symbol_kind::ENUM);
    let members: Vec<&str> = role.children.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(members, ["none", "vip"]);
    let bonus = &s[5];
    assert_eq!(bonus.children.len(), 2);
    let kill = &s[7];
    assert_eq!(kill.range.start, p(15, 0));
    assert_eq!(kill.range.end, p(23, 1));
    assert_eq!(kill.selection_range.start, p(15, 3));
}

#[test]
fn format_lays_out_a_script_canonically() {
    let messy = "rules \"T\" 1\ncarrier slayer\noption  x :int -5 .. 5=0\n\n\n\non kill( v,k ){ if k!=none{score(k,-1);score( k , x )}else{score(v,1)} // tail\n}\n";
    let want = "rules \"T\" 1\ncarrier slayer\noption x: int -5..5 = 0\n\non kill(v, k) {\n    if k != none {\n        score(k, -1)\n        score(k, x)\n    } else {\n        score(v, 1)\n    } // tail\n}\n";
    let got = format(messy).unwrap();
    assert_eq!(got, want);
    assert_eq!(format(want).unwrap(), want);
    assert!(format("rules \"T\" 1\non tick {\n").is_err());
    let table = "rules \"T\" 1\ncarrier slayer\ntable t: (a: int, b: int) = [\n(1, 2),\n  (3,4),\n]\nenum E {\n  a\n  b\n}\n";
    let want = "rules \"T\" 1\ncarrier slayer\ntable t: (a: int, b: int) = [\n    (1, 2),\n    (3, 4),\n]\nenum E {\n    a\n    b\n}\n";
    assert_eq!(format(table).unwrap(), want);
}

#[test]
fn every_builtin_is_clean_formats_idempotently_and_is_covered_by_tokens() {
    for (name, src) in scripts() {
        let bad: Vec<_> = diagnostics(src)
            .into_iter()
            .filter(|d| d.severity <= severity::WARNING)
            .collect();
        assert!(bad.is_empty(), "{name}: {bad:?}");

        let once = format(src).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(format(&once).unwrap(), once, "{name}: not idempotent");
        let before = crate::compile(src).unwrap();
        let after = crate::compile(&once).unwrap();
        assert_eq!(before.id(), after.id(), "{name}: the blob changed");

        let tokens = semantic_tokens(src);
        for w in tokens.windows(2) {
            let (a, b) = (w[0], w[1]);
            assert!(
                a.line < b.line || (a.line == b.line && a.start + a.len <= b.start),
                "{name}: {a:?} then {b:?}"
            );
        }
        let li = LineIndex::new(src);
        for t in scan::scan(src) {
            if !matches!(t.k, K::Ident | K::Num | K::Str | K::Comment) {
                continue;
            }
            let at = li.position(t.s);
            assert!(
                tokens
                    .iter()
                    .any(|k| k.line == at.line && k.start <= at.character && at.character < k.start + k.len),
                "{name}: no token covers {:?} at {at:?}",
                &src[t.s..t.e]
            );
        }

        let outline = symbols(src);
        let handlers = crate::compile(src).unwrap().handlers.len();
        let ons = outline.iter().filter(|s| s.kind == symbol_kind::EVENT).count();
        assert_eq!(ons, handlers, "{name}");
    }
}

#[test]
fn results_serialize_with_lsp_field_names() {
    let item = complete("rules \"T\" 1\non ", p(1, 3)).remove(0);
    let v = serde_json::to_value(&item).unwrap();
    for key in ["label", "kind", "detail", "documentation", "insertText", "insertTextFormat"] {
        assert!(v.get(key).is_some(), "{key}: {v}");
    }
    assert_eq!(v["documentation"]["kind"], "markdown");
    let v = serde_json::to_value(hover(SRC, p(12, 5)).unwrap()).unwrap();
    assert_eq!(v["contents"]["kind"], "markdown");
    assert_eq!(v["range"]["start"], serde_json::json!({"line": 12, "character": 4}));
    let v = serde_json::to_value(signature_help(SRC, p(12, 13)).unwrap()).unwrap();
    assert_eq!(v["activeParameter"], 1);
    assert_eq!(v["activeSignature"], 0);
    assert_eq!(v["signatures"][0]["parameters"][0], serde_json::json!({"label": "player"}));
    let v = serde_json::to_value(semantic_tokens(SRC)[0]).unwrap();
    assert_eq!(
        v,
        serde_json::json!({"line": 0, "start": 0, "length": 5, "type": 0, "typeName": "keyword"})
    );
    let v = serde_json::to_value(&symbols(SRC)[1]).unwrap();
    assert!(v.get("selectionRange").is_some());
    let v = serde_json::to_value(&diagnostics(SRC)[0]).unwrap();
    assert_eq!(v["severity"], 3);
    assert_eq!(v["source"], "rules");
}
