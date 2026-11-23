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
    assert_eq!(d[0].code, "name.unknown");
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
    // `k` is `player?`, and `player` where `k != none` is in force.
    let k = hover_text(SRC, 18, 15);
    assert!(k.contains("let k: player?"), "{k}");
    assert!(k.contains("Here `k: player` (not none here: checked on line 18)"), "{k}");
    assert!(!hover_text(SRC, 16, 8).contains("Here"));
    assert!(hover_text(SRC, 20, 40).contains("(argument) victim: player"));
    let vip = hover_text(SRC, 17, 37);
    assert!(vip.contains("Role.vip") && vip.contains("enum `Role`"), "{vip}");
    assert!(hover_text(SRC, 17, 24).contains("A variable of each player"));
    assert!(hover_text(SRC, 15, 4).contains("on kill(victim: player, killer: player?)"));
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
    assert_eq!(s.signatures[0].label, "kill(victim: player, killer: player?)");
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
    let items = complete(top, p(1, 0));
    let l = labels(&items);
    assert!(l.contains(&"option") && l.contains(&"global var") && l.contains(&"on kill"), "{l:?}");

    let on = "rules \"T\" 1\non ";
    let items = complete(on, p(1, 3));
    let kill = items.iter().find(|i| i.label == "kill").unwrap();
    assert_eq!(kill.insert_text, "kill(${1:victim}, ${2:killer}) {\n\t$0\n}");
    assert_eq!(kill.kind, kind::EVENT);

    assert!(labels(&complete("rules \"T\" 1\ncarrier ", p(1, 8))).contains(&"slayer"));
    assert!(labels(&complete("rules \"T\" 1\nrequires ", p(1, 9))).contains(&"player.team"));
    let forge_items = complete("rules \"T\" 1\nrequires forge.", p(1, 15));
    let forge = labels(&forge_items);
    assert!(forge.contains(&"place") && !forge.contains(&"player.team"), "{forge:?}");
    let type_items = complete("rules \"T\" 1\nenum Role { none, vip }\nplayer var role: \n", p(2, 17));
    let types = labels(&type_items);
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
    let labelled = semantic_tokens("rules \"T\" 1\ncarrier slayer\nlabel x: objects\non tick { for each x o { } }\n");
    assert!(labelled.iter().any(|t| t.line == 2 && t.start == 6 && t.kind == TokenKind::Label));
    assert!(labelled.iter().any(|t| t.line == 3 && t.start == 19 && t.kind == TokenKind::Label));
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
    for key in ["label", "kind", "detail", "documentation", "insertText", "insertTextFormat", "sortText"] {
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
    // A fix's edits are LSP's `TextEdit`s.
    let d = diagnostics("rules \"T\" 1\ncarrier slayer\nglobal var j: player = none\n");
    let v = serde_json::to_value(&d[0]).unwrap();
    assert_eq!(v["code"], "null.var");
    assert_eq!(v["fixes"][0]["edits"][0]["newText"], "?");
    assert_eq!(v["fixes"][0]["edits"][0]["range"]["start"], serde_json::json!({"line": 2, "character": 20}));
}

// ---------- the checker's reading: labels, narrowing, fixes ----------

const HEAD: &str = "rules \"T\" 1\ncarrier slayer\n";

fn script(requires: &str, body: &str) -> String {
    if requires.is_empty() {
        format!("{HEAD}{body}")
    } else {
        format!("{HEAD}requires {requires}\n{body}")
    }
}

/// `src` with `edits` made.
fn apply(src: &str, edits: &[TextEdit]) -> String {
    let li = LineIndex::new(src);
    let mut spans: Vec<(usize, usize, &str)> = edits
        .iter()
        .map(|e| (li.offset(e.range.start), li.offset(e.range.end), e.new_text.as_str()))
        .collect();
    spans.sort_by_key(|s| std::cmp::Reverse(s.0));
    let mut out = src.to_string();
    for (s, e, text) in spans {
        out.replace_range(s..e, text);
    }
    out
}

const LABELS: &str = "rules \"T\" 1
carrier slayer
label goal: zone
global var n: int = 0
on enter(goal, o) {
    n = n + 1
}
on drop(p, o) {
    if o is goal and inside(p, o) { n = n + 1 }
}
";

#[test]
fn a_labels_hover_says_what_a_map_needs() {
    let h = hover_text(LABELS, 2, 7);
    assert!(h.contains("label goal: zone"), "{h}");
    assert!(h.contains("One zone."), "{h}");
    assert!(h.contains("A map needs: exactly 1 zone labelled \"goal\"."), "{h}");
    assert!(h.contains("Used 2 times."), "{h}");
    // A use shows the declaration too.
    assert_eq!(hover_text(LABELS, 8, 13), h);
    // An API entry's hover is its front-end signature.
    assert!(hover_text(LABELS, 8, 25).contains("inside(player, zone) -> bool"));
    // A set label lists what its members have.
    let src = "rules \"T\" 1\ncarrier slayer\nlabel capture: zones numbered 0..1 = \"zone\"\non tick { for each capture c { } }\n";
    let h = hover_text(src, 2, 7);
    assert!(h.contains("A set of zones. Members have"), "{h}");
    assert!(h.contains("numbered 0..1"), "{h}");
}

#[test]
fn definition_references_and_rename_follow_a_label() {
    let at = |l: &[Location]| -> Vec<(u32, u32)> {
        l.iter().map(|x| (x.range.start.line, x.range.start.character)).collect()
    };
    // `goal` in `on enter(goal, o)` and in `o is goal` are the label's.
    assert_eq!(at(&definition(LABELS, p(8, 13))), vec![(2, 6)]);
    assert_eq!(at(&definition(LABELS, p(4, 10))), vec![(2, 6)]);
    assert_eq!(at(&references(LABELS, p(4, 10))), vec![(2, 6), (4, 9), (8, 12)]);

    // Renaming keeps the map's text.
    let edits = rename(LABELS, p(2, 7), "finish").unwrap();
    let renamed = apply(LABELS, &edits);
    assert!(renamed.contains("label finish: zone = \"goal\"\n"), "{renamed}");
    assert!(renamed.contains("on enter(finish, o)"), "{renamed}");
    assert!(renamed.contains("if o is finish and"), "{renamed}");
    let c = crate::diagnose(&renamed);
    assert!(c.ir.is_some(), "{:#?}", c.diagnostics);
    assert_eq!(c.ir.unwrap().labels[0].text, "goal");
    // Renaming back to the map's text drops the `= "goal"`.
    let back = apply(&renamed, &rename(&renamed, p(2, 7), "goal").unwrap());
    assert_eq!(back, LABELS);

    assert!(rename(LABELS, p(2, 7), "n").is_err(), "a global is named n");
    assert!(rename(LABELS, p(2, 7), "if").is_err(), "a keyword");
    assert!(rename(LABELS, p(2, 7), "9lives").is_err(), "not a name");
    assert!(rename(LABELS, p(0, 1), "x").is_err(), "nothing to rename");
    // A local renames at its declaration and its uses only.
    let edits = rename(SRC, p(18, 15), "lead").unwrap();
    let ranges: Vec<(u32, u32)> = edits.iter().map(|e| (e.range.start.line, e.range.start.character)).collect();
    assert_eq!(ranges, [(16, 8), (17, 7), (17, 21), (18, 15)]);
}

#[test]
fn semantic_tokens_mark_labels_and_type_words() {
    let tokens = semantic_tokens(LABELS);
    let at = |line: u32, start: u32| -> TokenKind {
        tokens
            .iter()
            .find(|t| t.line == line && t.start == start)
            .unwrap_or_else(|| panic!("no token at {line}:{start}"))
            .kind
    };
    assert_eq!(at(2, 0), TokenKind::Keyword);
    assert_eq!(at(2, 6), TokenKind::Label);
    assert_eq!(at(2, 12), TokenKind::Type);
    assert_eq!(at(4, 9), TokenKind::Label);
    assert_eq!(at(8, 9), TokenKind::Keyword);
    assert_eq!(at(8, 12), TokenKind::Label);
    assert_eq!(TokenKind::Label.index(), 13);
    let numbered = semantic_tokens("rules \"T\" 1\ncarrier slayer\nlabel cap: zones numbered 0..1 = \"zone\"\n");
    let kind = |start: u32| numbered.iter().find(|t| t.line == 2 && t.start == start).map(|t| t.kind);
    assert_eq!(kind(11), Some(TokenKind::Type));
    assert_eq!(kind(17), Some(TokenKind::Keyword));
}

const WANT: &str = "rules \"T\" 1
carrier slayer
label goal: zone
label pad: object
label bonus: zone?
label capture: zones numbered 0..1
on spawn(p) {
    let z = goal
    if inside(p, goal) { }
}
on drop(q, o) {
    if o is pad { }
}
";

#[test]
fn completion_puts_what_the_place_wants_first() {
    // `inside(p, |)` wants a zone: zone labels, zone values, values that
    // may be none, set labels to index, then the rest.
    let items = complete(WANT, p(8, 17));
    let l = labels(&items);
    assert_eq!(&l[..4], ["goal", "z", "bonus", "capture"], "{l:?}");
    assert!(items[2].detail.contains("may be none"), "{:?}", items[2]);
    assert_eq!(items[3].insert_text, "capture[");
    assert!(l.contains(&"score") && l.contains(&"p"), "nothing is hidden: {l:?}");
    assert_eq!(l.last(), Some(&"pad"));
    assert_eq!(items[0].sort_text, "0000");
    assert_eq!(items[1].sort_text, "0001");

    // After `is`: the labels.
    assert_eq!(labels(&complete(WANT, p(11, 12))), ["goal", "pad", "bonus", "capture"]);
    // A handler's label place: zone labels.
    let on = "rules \"T\" 1\nlabel goal: zone\nlabel pad: object\non enter(";
    assert_eq!(labels(&complete(on, p(3, 9))), ["goal"]);
    // A `label` line: whole flag labels, then kinds, then requirements.
    let flags = complete("rules \"T\" 1\nlabel ", p(1, 6));
    let hill = flags.iter().find(|i| i.label == "label hill_corner: zones numbered").unwrap();
    assert_eq!(hill.insert_text, "hill_corner: zones numbered");
    assert!(labels(&complete("rules \"T\" 1\nlabel cap: ", p(1, 11))).contains(&"zones"));
    let reqs = complete("rules \"T\" 1\nlabel cap: zones ", p(1, 17));
    assert!(labels(&reqs).contains(&"numbered") && labels(&reqs).contains(&"per team"));
}

/// One script per fix: the diagnostic it answers, and each of its fixes
/// applied gives a script without it.
#[test]
fn each_fix_applied_removes_its_diagnostic() {
    let cases: &[(&str, &str, &str, usize)] = &[
        ("", "label barrier: objects\non tick { for each barier b { } }\n", "label.unknown", 2),
        ("objects.delete", "label barrier: objects\non tick {\n    delete(barrier)\n}\n", "label.set-as-one", 1),
        ("", "label core: object\non tick { for each core c { } }\n", "label.one-as-set", 1),
        ("", "label pad: object\non tick { for each players p where inside(p, pad) { } }\n", "label.kind", 1),
        ("", "label hill: zones\non tick { if let z = hill[0] { } }\n", "label.index", 1),
        ("", "label a: zones\n", "label.unused", 1),
        ("", "global var n: int = 0\non kill(victim, killer) {\n    n = killer.kills\n}\n", "null.use", 1),
        (
            "",
            "global var n: int = 0\non kill(victim, killer) {\n    if killer.kills > 2 or n > 3 {\n        n = 1\n    }\n}\n",
            "null.use",
            1,
        ),
        ("", "on kill(victim, killer) {\n    score(killer, 1)\n}\n", "null.arg", 1),
        ("", "on spawn(p) {\n    score(first(players q where q.alive), 1)\n}\n", "null.arg", 1),
        ("", "global var j: player = none\n", "null.var", 1),
        ("", "on spawn(p) {\n    if p != none and p.alive {\n        score(p, 1)\n    }\n}\n", "null.never", 1),
        (
            "player.weapons",
            "table t: (w: tag) = [(\"a/b\")]\non spawn(p) { give(p, t[0].w) }\n",
            "tag.group",
            1,
        ),
        ("", "on spawn(p) { set_team(p, team 1) }\n", "cap.missing", 1),
        ("player.weapons", "on spawn(p) { set_team(p, team 1) }\n", "cap.missing", 1),
        ("player.team", "on spawn(p) { }\n", "cap.unused", 1),
        ("player.team, player.weapons", "on spawn(p) { give(p, \"objects/w\") }\n", "cap.unused", 1),
    ];
    for &(requires, body, code, count) in cases {
        let src = script(requires, body);
        let d = diagnostics(&src);
        let found = d
            .iter()
            .find(|x| x.code == code)
            .unwrap_or_else(|| panic!("no {code} in {d:#?}\nfor\n{src}"));
        assert_eq!(found.fixes.len(), count, "{code}: {:#?}", found.fixes);
        for f in &found.fixes {
            let fixed = apply(&src, &f.edits);
            let after = crate::diagnose(&fixed);
            assert!(
                !after.diagnostics.iter().any(|x| x.code == code),
                "{code}: \"{}\" leaves it in\n{fixed}\n{:#?}",
                f.title,
                after.diagnostics
            );
        }
    }
}

#[test]
fn code_actions_are_the_fixes_at_a_range() {
    let src = script("", "label barrier: objects\non tick { for each barier b { } }\n");
    let at = Range { start: p(3, 21), end: p(3, 21) };
    let actions = actions(&src, at);
    let titles: Vec<&str> = actions.iter().map(|a| a.title.as_str()).collect();
    assert_eq!(titles, ["Change to `barrier`", "Declare label `barier`"]);
    assert!(actions.iter().all(|a| a.kind == "quickfix" && a.diagnostic.code == "label.unknown"));
    assert!(actions[0].diagnostic.fixes.is_empty());
    let fixed = apply(&src, &actions[0].edits);
    assert!(fixed.contains("for each barrier b"), "{fixed}");
    // Nothing to fix elsewhere.
    assert!(super::actions(&src, Range { start: p(0, 0), end: p(0, 1) }).is_empty());
}

#[test]
fn every_diagnostic_is_reported_with_its_code() {
    let src = script(
        "",
        "global var j: player = none
on kill(victim, killer) {
    score(killer, 1)
    score(nobody, 1)
}
",
    );
    let codes: Vec<&str> = diagnostics(&src).iter().map(|d| d.code).collect();
    assert_eq!(codes, ["null.var", "null.arg", "name.unknown"]);
}

#[test]
fn else_if_let_is_read_like_else_if() {
    let src = "rules \"T\" 1
carrier slayer
on kill(victim, killer) {
    if killer == none {
        score(victim, 1)
    } else if let mate = first(players p where p.team == killer.team) {
        score(mate, 1)
    }
}
";
    assert!(crate::diagnose(src).ir.is_some());
    let at = |l: &[Location]| -> Vec<(u32, u32)> {
        l.iter().map(|x| (x.range.start.line, x.range.start.character)).collect()
    };
    assert_eq!(at(&definition(src, p(6, 14))), vec![(5, 18)]);
    // The `else` holds what `killer == none` false proves.
    let h = hover_text(src, 5, 58);
    assert!(h.contains("(argument) killer: player?"), "{h}");
    assert!(h.contains("not none here: checked on line 4"), "{h}");
}

/// The model reads every type from the checker: it walks expressions to
/// resolve names, and no walk gives a type of its own.
#[test]
fn the_model_holds_no_type_inference() {
    let model = include_str!("model.rs");
    for banned in [
        "BinOp::Add",
        "BinOp::Sub",
        "BinOp::Mul",
        "BinOp::Div",
        "BinOp::Rem",
        "crate::ty::Ty",
        "fn literal",
    ] {
        assert!(!model.contains(banned), "ide/model.rs works out a type: {banned}");
    }
    assert!(model.contains("fn expr(&mut self, e: &Expr, env: &mut Env) {"), "expr gives no type");
    assert!(model.contains("self.checked.ty(e)"));
}

const SLICE5: &str = "rules \"T\" 1
carrier slayer
const hold = 30s
enum Role { none, vip }
player var role: Role = Role.none
fn leads(p: player) -> bool = p.score > 2
on spawn(p) {
    match p.role {
        Role.vip { score(p, whole_seconds(hold)) }
        else { }
    }
    if variant.engine == carrier.slayer and leads(p) { score(p, ticks(5s)) }
}
";

/// Time literals, constants, `match`, value functions and the `carrier`
/// enum: read, hovered, completed and kept whole by the formatter.
#[test]
fn time_const_match_and_carrier_are_read() {
    let errors: Vec<String> = diagnostics(SLICE5)
        .iter()
        .filter(|d| d.severity == severity::ERROR)
        .map(|d| format!("{}: {}", d.code, d.message))
        .collect();
    assert!(errors.is_empty(), "{errors:?}");

    // A time literal is one token.
    let nums: Vec<&str> = scan::scan(SLICE5)
        .into_iter()
        .filter(|t| t.k == K::Num)
        .map(|t| &SLICE5[t.s..t.e])
        .collect();
    assert!(nums.contains(&"30s") && nums.contains(&"5s"), "{nums:?}");
    let tokens = semantic_tokens(SLICE5);
    assert!(tokens.iter().any(|t| t.line == 2 && t.start == 13 && t.len == 3), "{tokens:?}");
    let formatted = format(SLICE5).unwrap();
    assert!(formatted.contains("const hold = 30s") && formatted.contains("ticks(5s)"), "{formatted}");

    let h = hover_text(SLICE5, 2, 7);
    assert!(h.contains("A constant of type time") && h.contains("900 ticks"), "{h}");
    let h = hover_text(SLICE5, 2, 14);
    assert!(h.contains("900 ticks"), "{h}");
    let h = hover_text(SLICE5, 8, 30);
    assert!(h.contains("whole seconds"), "{h}");
    let h = hover_text(SLICE5, 11, 35);
    assert!(h.contains("engine index"), "{h}");
    let h = hover_text(SLICE5, 11, 45);
    assert!(h.contains("fn leads(p: player) -> bool"), "{h}");

    // The const's use and the arm's enum go to their declarations.
    let d = definition(SLICE5, p(8, 43));
    assert_eq!(d[0].range.start, p(2, 6));
    let d = definition(SLICE5, p(8, 9));
    assert_eq!(d[0].range.start, p(3, 5));

    let carriers = complete(SLICE5, p(11, 33));
    let names = labels(&carriers);
    assert!(names.contains(&"slayer") && names.contains(&"assault"), "{names:?}");
    let names: Vec<String> = symbols(SLICE5).into_iter().map(|s| s.name).collect();
    assert!(names.contains(&"hold".to_string()), "{names:?}");
    let top = complete("rules \"T\" 1\ncarrier slayer\n\n", p(2, 0));
    assert!(labels(&top).contains(&"const"));
}

#[test]
fn highlights_mark_the_declaration_and_assignments_as_writes() {
    let marks = |pos: Position| -> Vec<(u32, u32, u8)> {
        highlights(LABELS, pos)
            .iter()
            .map(|h| (h.range.start.line, h.range.start.character, h.kind))
            .collect()
    };
    let (r, w) = (highlight_kind::READ, highlight_kind::WRITE);
    assert_eq!(
        marks(p(5, 4)),
        vec![(3, 11, w), (5, 4, w), (5, 8, r), (8, 36, w), (8, 40, r)]
    );
    // A label's uses are reads, in source order.
    assert_eq!(marks(p(8, 13)), vec![(2, 6, w), (4, 9, r), (8, 12, r)]);
    assert!(highlights(LABELS, p(0, 1)).is_empty());
}

#[test]
fn prepare_rename_names_what_a_rename_would_rename() {
    let r = prepare_rename(LABELS, p(8, 13)).unwrap();
    assert_eq!(r.placeholder, "goal");
    assert_eq!((r.range.start, r.range.end), (p(8, 12), p(8, 16)));
    let r = prepare_rename(LABELS, p(5, 5)).unwrap();
    assert_eq!(r.placeholder, "n");
    assert!(prepare_rename(LABELS, p(0, 1)).is_err(), "nothing to rename");
    let pal = format!("{HEAD}palette \"guns\" = [\"objects/weapons/rifle/battle_rifle/battle_rifle\"]\n");
    let line = pal.lines().position(|l| l.starts_with("palette")).unwrap() as u32;
    if let Ok(r) = prepare_rename(&pal, p(line, 10)) {
        panic!("a palette is renamed by its string: {r:?}");
    }
}

#[test]
fn folding_ranges_cover_blocks_comment_runs_and_regions() {
    let src = "// A script.
// Two lines.
rules \"T\" 1
carrier slayer
// region scoring
table t: (a: int, b: int) = [
    (1, 2),
]
// endregion
on kill(v, k) {
    if k != none {
        score(k, 1)
    }
}
";
    let folds: Vec<(u32, u32, Option<&str>)> = folding_ranges(src)
        .iter()
        .map(|f| (f.start_line, f.end_line, f.kind))
        .collect();
    assert_eq!(
        folds,
        vec![
            (0, 1, Some(fold_kind::COMMENT)),
            (4, 8, Some(fold_kind::REGION)),
            (5, 6, None),
            (9, 12, None),
            (10, 11, None),
        ]
    );
    // A block left open folds nothing; a one-line comment is no run.
    assert!(folding_ranges("rules \"T\" 1\n// one\non tick {\n    let a = 1\n").is_empty());
}

#[test]
fn format_range_edits_only_the_lines_it_touches() {
    let src = "rules \"T\" 1\ncarrier slayer\noption  x :int 1..5=1\non kill(v, k) {\n    score(k,  x)\n}\n";
    let edits = format_range(src, Range { start: p(4, 0), end: p(4, 3) }).unwrap();
    assert_eq!(edits.len(), 1, "{edits:?}");
    assert_eq!((edits[0].range.start, edits[0].range.end), (p(4, 0), p(5, 0)));
    assert_eq!(edits[0].new_text, "    score(k, x)\n");
    let edits = format_range(src, Range { start: p(2, 0), end: p(2, 0) }).unwrap();
    assert_eq!(apply(src, &edits), src.replace("option  x :int 1..5=1", "option x: int 1..5 = 1"));
    // Both, applied, are the whole format.
    let all = format_range(src, Range { start: p(0, 0), end: p(5, 1) }).unwrap();
    assert_eq!(apply(src, &all), format(src).unwrap());
    // A clean line in range, or a source that does not parse.
    assert!(format_range(src, Range { start: p(0, 0), end: p(1, 0) }).unwrap().is_empty());
    assert!(format_range("rules \"T\" 1\non tick {\n", Range::default()).is_err());
}

#[test]
fn offsets_count_utf16_units_for_incremental_changes() {
    let src = "a\u{1F600}b\nc";
    assert_eq!(offset(src, p(0, 3)), 5);
    assert_eq!(offset(src, p(1, 0)), 7);
    assert_eq!(offset(src, p(0, 99)), 6);
    assert_eq!(offset(src, p(9, 0)), src.len());
}

// ---------- the settings block, quick marks and the scorer ----------

const SWAT: &str = "rules \"Swat\" 1
carrier slayer
scoring carrier
about \"No shields.\"
quick score_limit, shields
settings {
    score_limit = 50
    shields = none
}
";

/// `src`'s `code` diagnostic, and the script each of its fixes makes, by
/// the fix's title. Each result is checked to be without the diagnostic.
fn fixed(src: &str, code: &str) -> Vec<(String, String)> {
    let d = diagnostics(src);
    let found = d
        .iter()
        .find(|x| x.code == code)
        .unwrap_or_else(|| panic!("no {code} in {d:#?}\nfor\n{src}"));
    found
        .fixes
        .iter()
        .map(|f| {
            let out = apply(src, &f.edits);
            let after = crate::diagnose(&out);
            assert!(
                !after.diagnostics.iter().any(|x| x.code == code),
                "{code}: \"{}\" leaves it in\n{out}\n{:#?}",
                f.title,
                after.diagnostics
            );
            (f.title.clone(), out)
        })
        .collect()
}

#[test]
fn a_settings_script_is_clean() {
    let bad: Vec<_> = diagnostics(SWAT)
        .into_iter()
        .filter(|d| d.severity <= severity::WARNING)
        .collect();
    assert!(bad.is_empty(), "{bad:?}");
}

#[test]
fn completion_in_the_settings_block_offers_the_carriers_settings() {
    let src = "rules \"Swat\" 1\ncarrier slayer\nscoring carrier\nsettings {\n    score_limit = 50\n    \n}\n";
    let items = complete(src, p(5, 4));
    let l = labels(&items);
    assert!(l.contains(&"shields") && l.contains(&"starting_weapon") && l.contains(&"lives"), "{l:?}");
    assert!(!l.contains(&"score_limit"), "a setting the block has is not offered again");
    assert!(
        !l.contains(&"juggernaut_movement") && !l.contains(&"oddball_ball_count") && !l.contains(&"ctf_flag_type"),
        "{l:?}"
    );
    let shields = items.iter().find(|i| i.label == "shields").unwrap();
    assert_eq!(shields.kind, kind::PROPERTY);
    assert_eq!(shields.insert_text, "shields = ${1|normal,none,overshields|}");
    assert_eq!(shields.insert_text_format, SNIPPET);
    assert!(shields.documentation.value.contains("Shields") && shields.documentation.value.contains("every game type"));
    let sensor = items.iter().find(|i| i.label == "motion_sensor").unwrap();
    assert_eq!(sensor.insert_text, "motion_sensor = ${1|true,false|}");
    let others = complete(&src.replace("score_limit", "lives"), p(5, 4));
    assert!(labels(&others).contains(&"score_limit"));

    // A game type's own settings are offered on its carrier only.
    let jug = src.replace("slayer", "juggernaut");
    assert!(labels(&complete(&jug, p(5, 4))).contains(&"juggernaut_movement"));

    // After a `;` on a line, and with the block left open and the word
    // half typed (the carrier is read from the tokens).
    let semi = "rules \"Swat\" 1\ncarrier slayer\nscoring carrier\nsettings {\n    lives = 3; \n}\n";
    assert!(labels(&complete(semi, p(4, 15))).contains(&"shields"));
    let open = "rules \"Swat\" 1\ncarrier slayer\nscoring carrier\nsettings {\n    sc";
    let items = complete(open, p(4, 6));
    let l = labels(&items);
    assert!(l.contains(&"score_limit") && !l.contains(&"juggernaut_movement"), "{l:?}");

    // After a value is typed the place wants nothing.
    let done = "rules \"Swat\" 1\ncarrier slayer\nscoring carrier\nsettings {\n    lives = 3 \n}\n";
    assert!(complete(done, p(4, 14)).is_empty());
}

#[test]
fn completion_after_an_equals_offers_the_settings_values() {
    let at = |setting: &str| {
        let src = format!(
            "rules \"T\" 1\ncarrier slayer\nscoring carrier\nsettings {{\n    {setting} = \n    lives = 3\n}}\n"
        );
        complete(&src, p(4, (4 + setting.len() + 3) as u32))
    };
    assert_eq!(labels(&at("shields")), ["normal", "none", "overshields"]);
    assert_eq!(labels(&at("motion_sensor")), ["true", "false"]);
    assert_eq!(labels(&at("score_limit")), ["number", "none"]);
    let weapons = at("starting_weapon");
    assert!(labels(&weapons).contains(&"battle_rifle") && labels(&weapons).contains(&"magnum"));
    let rifle = weapons.iter().find(|i| i.label == "battle_rifle").unwrap();
    assert_eq!(
        (rifle.kind, rifle.detail.as_str(), rifle.insert_text.as_str()),
        (kind::ENUM_MEMBER, "Battle Rifle", "battle_rifle")
    );
    let time = at("time_limit");
    let seconds = time.iter().find(|i| i.label == "seconds").unwrap();
    assert!(seconds.insert_text.ends_with("}s"), "{seconds:?}");
    assert!(labels(&time).contains(&"none"));
    assert!(at("no_such_setting").is_empty());
}

#[test]
fn completion_after_quick_offers_options_then_settings() {
    // An option first, then the block's settings, then the rest of the
    // carrier's record settings; a name marked already is not offered.
    let src = "rules \"T\" 1\ncarrier slayer\nquick lives, points\noption points: int 1..5 = 2 \"Points\"\non tick { }\n";
    let items = complete(src, p(2, 13));
    let l = labels(&items);
    assert_eq!(l[0], "points");
    assert_eq!(items[0].kind, kind::CONSTANT);
    assert!(l.contains(&"shields") && !l.contains(&"lives"), "{l:?}");
    assert!(!l.contains(&"gravity"), "an extra of More Options is no Quick Options row");
    assert!(!l.contains(&"juggernaut_movement"));

    let items = complete(SWAT, p(4, 19));
    let l = labels(&items);
    assert_eq!(&l[..2], ["shields", "team_play"], "{l:?}");
    assert!(!l.contains(&"score_limit"));

    // `scoring` is followed by `carrier`, and by nothing after that.
    assert_eq!(labels(&complete("rules \"T\" 1\ncarrier slayer\nscoring \n", p(2, 8))), ["carrier"]);
    assert!(complete("rules \"T\" 1\ncarrier slayer\nscoring carrier \n", p(2, 16)).is_empty());
}

#[test]
fn the_header_words_are_offered_as_snippets() {
    let items = complete("rules \"T\" 1\n\n", p(1, 0));
    let l = labels(&items);
    for w in ["scoring", "about", "quick", "settings"] {
        assert!(l.contains(&w), "{w}: {l:?}");
    }
    let block = items.iter().find(|i| i.label == "settings").unwrap();
    assert_eq!(block.insert_text, "settings {\n\t${1:score_limit} = ${2:50}\n}");
    assert_eq!(block.insert_text_format, SNIPPET);
    assert!(block.documentation.value.contains("record settings"));
    let scoring = items.iter().find(|i| i.label == "scoring").unwrap();
    assert_eq!(scoring.insert_text, "scoring carrier");
}

#[test]
fn hover_on_a_setting_names_its_label_carriers_and_values() {
    // In the block, and on the mark that names it.
    let h = hover_text(SWAT, 6, 6);
    assert!(h.contains("score_limit: int") && h.contains("Score to Win"), "{h}");
    assert!(h.contains("Applies to every game type") && h.contains("A number"), "{h}");
    let h = hover_text(SWAT, 4, 20);
    assert!(
        h.contains("Shields") && h.contains("`none` (None)") && h.contains("`overshields` (Overshields)"),
        "{h}"
    );
    assert_eq!(hover(SWAT, p(4, 20)).unwrap().range, Range { start: p(4, 19), end: p(4, 26) });
    // A marked setting the block does not set.
    let lives = "rules \"T\" 1\ncarrier slayer\nscoring carrier\nquick lives\n";
    assert!(hover_text(lives, 3, 7).contains("Lives per Round"));
    // A setting of another game type.
    let jug = "rules \"T\" 1\ncarrier slayer\nscoring carrier\nsettings {\n    juggernaut_movement = fast\n}\n";
    let h = hover_text(jug, 4, 8);
    assert!(h.contains("Applies to Juggernaut") && h.contains("carrier is Slayer, so it cannot name"), "{h}");
    // The four words.
    assert!(hover_text(SWAT, 5, 2).contains("record settings"));
    assert!(hover_text(SWAT, 4, 2).contains("Quick Options"));
    assert!(hover_text(SWAT, 2, 3).contains("`scoring carrier`"));
    assert!(hover_text(SWAT, 3, 2).contains("description"));
}

#[test]
fn a_mark_goes_to_its_settings_line_or_its_option() {
    let at = |l: &[Location]| -> Vec<(u32, u32)> {
        l.iter().map(|x| (x.range.start.line, x.range.start.character)).collect()
    };
    assert_eq!(at(&definition(SWAT, p(4, 20))), vec![(7, 4)]);
    assert_eq!(at(&references(SWAT, p(7, 5))), vec![(7, 4), (4, 19)]);
    let opt = "rules \"T\" 1\ncarrier slayer\nquick points\noption points: int 1..5 = 2 \"Points\"\non kill(v, k) { score(k, points) }\n";
    assert_eq!(at(&definition(opt, p(2, 8))), vec![(3, 7)]);
}

#[test]
fn renaming_an_option_renames_its_quick_mark() {
    let src = "rules \"T\" 1\ncarrier slayer\nquick points, lives\noption points: int 1..5 = 2 \"Points\"\non kill(v, k) { score(k, points) }\n";
    for pos in [p(2, 8), p(3, 9), p(4, 27)] {
        let edits = rename(src, pos, "reward").unwrap();
        assert_eq!(edits.len(), 3, "{edits:?}");
        assert_eq!(apply(src, &edits), src.replace("points", "reward"), "{pos:?}");
    }
    // An option cannot take a setting's name; a setting is not renamed.
    assert!(rename(src, p(3, 9), "lives").unwrap_err().contains("setting's name"));
    assert!(rename(SWAT, p(6, 5), "points").unwrap_err().contains("cannot be renamed"));
    assert!(prepare_rename(SWAT, p(7, 5)).is_err());
    assert!(prepare_rename(src, p(2, 8)).is_ok());
}

#[test]
fn each_settings_fix_removes_its_diagnostic() {
    let head = "rules \"T\" 1\ncarrier slayer\nscoring carrier\n";
    let block = |lines: &str| format!("{head}settings {{\n{lines}}}\n");

    // setting.unknown: the nearest name.
    let r = fixed(&block("    score_limt = 50\n"), "setting.unknown");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].0, "Change to `score_limit`");
    assert_eq!(r[0].1, block("    score_limit = 50\n"));

    // setting.carrier, in the block and on the `quick` line.
    let r = fixed(&block("    lives = 3\n    juggernaut_movement = fast\n"), "setting.carrier");
    assert_eq!(r[0].1, block("    lives = 3\n"));
    let r = fixed(&format!("{head}quick juggernaut_movement, lives\n"), "setting.carrier");
    assert_eq!(r[0].0, "Remove `juggernaut_movement` from `quick`");
    assert_eq!(r[0].1, format!("{head}quick lives\n"));
    // Two on a line.
    let r = fixed(&block("    lives = 3; juggernaut_movement = fast\n"), "setting.carrier");
    assert!(!r[0].1.contains("juggernaut_movement") && r[0].1.contains("lives = 3"), "{}", r[0].1);

    // setting.twice: the repeat goes.
    let r = fixed(&block("    lives = 3\n    lives = 4\n"), "setting.twice");
    assert_eq!(r[0].1, block("    lives = 3\n"));

    // quick.many: the sixth, or everything past the fifth.
    let five = "score_limit, lives, shields, rounds, team_play";
    let r = fixed(&format!("{head}quick {five}, time_limit\n"), "quick.many");
    assert_eq!(r[0].0, "Drop the sixth mark");
    assert_eq!(r[0].1, format!("{head}quick {five}\n"));
    let r = fixed(&format!("{head}quick {five}, time_limit, max_players\n"), "quick.many");
    assert_eq!(r[0].0, "Drop the 2 marks past the fifth");
    assert_eq!(r[0].1, format!("{head}quick {five}\n"));

    // quick.unknown: the nearest name, or no mark; quick.twice.
    let r = fixed(&format!("{head}quick lives, score_limt\n"), "quick.unknown");
    let titles: Vec<&str> = r.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(titles, ["Change to `score_limit`", "Remove `score_limt` from `quick`"]);
    assert_eq!(r[0].1, format!("{head}quick lives, score_limit\n"));
    assert_eq!(r[1].1, format!("{head}quick lives\n"));
    let r = fixed(&format!("{head}quick score_limt\n"), "quick.unknown");
    assert_eq!(r[1].1, head);
    let r = fixed(&format!("{head}quick lives, lives\n"), "quick.twice");
    assert_eq!(r[0].1, format!("{head}quick lives\n"));

    // scoring.none: add the line after the carrier's.
    let none = "rules \"T\" 1\ncarrier slayer\nsettings {\n    lives = 3\n}\n";
    let r = fixed(none, "scoring.none");
    assert_eq!(r[0].0, "Add `scoring carrier`");
    assert_eq!(r[0].1, "rules \"T\" 1\ncarrier slayer\nscoring carrier\nsettings {\n    lives = 3\n}\n");

    // scoring.handlers: the script scores, so the line goes.
    let r = fixed("rules \"T\" 1\ncarrier slayer\nscoring carrier\non tick { }\n", "scoring.handlers");
    assert_eq!(r[0].1, "rules \"T\" 1\ncarrier slayer\non tick { }\n");

    // list.carrier: the `list` line goes.
    let r = fixed("rules \"T\" 1\ncarrier oddball\nlist party\nscoring carrier\n", "list.carrier");
    assert_eq!(r[0].1, "rules \"T\" 1\ncarrier oddball\nscoring carrier\n");
}

#[test]
fn the_formatter_lays_out_the_settings_block_and_the_header_lines() {
    let messy = "rules \"Swat\" 1\ncarrier slayer\nscoring   carrier\nabout  \"No shields.\"\nquick score_limit,shields\n\
                 settings {\nscore_limit=50;shields = none\n   time_limit =   300s\n}\n";
    let clean = "rules \"Swat\" 1\ncarrier slayer\nscoring carrier\nabout \"No shields.\"\nquick score_limit, shields\n\
                 settings {\n    score_limit = 50\n    shields = none\n    time_limit = 300s\n}\n";
    let out = format(messy).unwrap();
    assert_eq!(out, clean);
    assert_eq!(format(&out).unwrap(), out);
    assert_eq!(crate::compile(messy).unwrap().id(), crate::compile(&out).unwrap().id());
    // A block on one line goes one setting a line.
    let one = "rules \"T\" 1\ncarrier slayer\nscoring carrier\nsettings { lives = 3 }\n";
    assert_eq!(
        format(one).unwrap(),
        "rules \"T\" 1\ncarrier slayer\nscoring carrier\nsettings {\n    lives = 3\n}\n"
    );
    // A range over a setting line formats that line.
    let edits = format_range(messy, Range { start: p(7, 0), end: p(7, 1) }).unwrap();
    assert!(!edits.is_empty());
}

#[test]
fn the_settings_block_folds_and_the_header_words_are_keywords() {
    let folds: Vec<(u32, u32)> = folding_ranges(SWAT).iter().map(|f| (f.start_line, f.end_line)).collect();
    assert_eq!(folds, vec![(5, 7)]);

    let tokens = semantic_tokens(SWAT);
    let kind_at = |line: u32, start: u32| tokens.iter().find(|t| t.line == line && t.start == start).map(|t| t.kind);
    for (line, start) in [(2, 0), (3, 0), (4, 0), (5, 0)] {
        assert_eq!(kind_at(line, start), Some(TokenKind::Keyword), "{line}:{start}");
    }
    assert_eq!(kind_at(6, 4), Some(TokenKind::Variable), "a setting's name");
    assert_eq!(kind_at(4, 19), Some(TokenKind::Variable), "a mark");
    // In a handler the four words are names.
    let src = "rules \"T\" 1\ncarrier slayer\non tick {\n    let about = 1\n    let quick = about + 1\n}\n";
    let tokens = semantic_tokens(src);
    assert!(tokens.iter().filter(|t| t.line >= 3).all(|t| t.kind != TokenKind::Keyword || t.len == 3));
}
