//! The script editor's pure helpers: positions, colour spans, problem
//! rows, completion text and the reload's params and row.

use super::*;
use agent_proto::rules::Reloaded;

#[test]
fn positions_and_bytes_round_trip_with_utf16_columns() {
    let text = "ab\n\u{e9}x\n\u{1f600}y";
    for byte in [0, 1, 2, 3, 5, 6, 7, 11, 12] {
        assert_eq!(byte_at(text, position_at(text, byte)), byte, "byte {byte}");
    }
    // The emoji is two UTF-16 units and four bytes.
    assert_eq!(position_at(text, 11), Position { line: 2, character: 2 });
    assert_eq!(byte_at(text, Position { line: 2, character: 2 }), 11);
    // Past the line's end: its end; past the text: its end.
    assert_eq!(byte_at(text, Position { line: 0, character: 40 }), 2);
    assert_eq!(byte_at(text, Position { line: 9, character: 0 }), text.len());
}

#[test]
fn tokens_become_byte_spans_of_their_lines() {
    let text = "on kill\n  \u{e9}t = 1";
    let tokens = [
        Token { line: 0, start: 0, len: 2, kind: TokenKind::Keyword },
        Token { line: 0, start: 3, len: 4, kind: TokenKind::Event },
        // UTF-16 column 2 is byte 2; the two-byte é makes its end byte 5.
        Token { line: 1, start: 2, len: 2, kind: TokenKind::Variable },
        // Cut to the line's end.
        Token { line: 1, start: 7, len: 9, kind: TokenKind::Number },
        // Past the text: left out.
        Token { line: 5, start: 0, len: 1, kind: TokenKind::Number },
    ];
    assert_eq!(
        spans(text, &tokens),
        [
            Span { line: 0, start: 0, end: 2, kind: TokenKind::Keyword },
            Span { line: 0, start: 3, end: 7, kind: TokenKind::Event },
            Span { line: 1, start: 2, end: 5, kind: TokenKind::Variable },
            Span { line: 1, start: 8, end: 9, kind: TokenKind::Number },
        ]
    );
}

#[test]
fn the_real_tokens_of_a_builtin_colour_its_keywords() {
    let src = rules::builtin::SLAYER;
    let s = spans(src, &ide::semantic_tokens(src));
    let first = s.first().expect("tokens");
    let line = src.lines().next().unwrap();
    assert_eq!(&line[first.start..first.end], "rules");
    assert_eq!(first.kind, TokenKind::Keyword);
}

#[test]
fn problems_list_errors_first_at_the_place_they_point_to() {
    let text = "rules \"x\" 1\ncarrier slayer\n\non kill(victim, killer) {\n    score(victim, nope)\n}\n";
    let rows = problems(text, &ide::diagnostics(text));
    let first = rows.first().expect("a problem");
    assert_eq!(first.severity, "error");
    assert!(first.message.contains("nope"), "{first:?}");
    assert_eq!(first.line, 5);
    assert_eq!(position_at(text, first.at).line, 4, "a click goes to the problem's line");
    let severities: Vec<_> = rows.iter().map(|r| r.severity).collect();
    let mut sorted = severities.clone();
    sorted.sort_by_key(|s| ["error", "warning", "info", "hint"].iter().position(|x| x == s));
    assert_eq!(severities, sorted);
}

#[test]
fn snippets_insert_their_defaults_and_words_end_at_the_cursor() {
    let item = ide::CompletionItem {
        label: "score".into(),
        kind: ide::kind::FUNCTION,
        detail: String::new(),
        documentation: ide::Markup::markdown(""),
        insert_text: "score(${1:p}, ${2:n})$0".into(),
        insert_text_format: ide::SNIPPET,
        sort_text: String::new(),
    };
    assert_eq!(plain_insert(&item), "score(p, n)");
    let text = "    sco";
    assert_eq!(word_start(text, text.len()), 4);
    assert_eq!(word_start(text, 4), 4);
    let mut ed = RulesEditor { text: text.into(), cursor: text.len(), ..Default::default() };
    ed.complete_with(&item);
    assert_eq!(ed.insert, Some((4, "score(p, n)".into())));
}

#[test]
fn a_reload_sends_the_checked_blob_and_names_its_hash() {
    let (params, hash) = reload_params(rules::builtin::SLAYER, ReloadMode::Auto).unwrap();
    let ir = rules::compile(rules::builtin::SLAYER).unwrap();
    assert_eq!(hash, ir.id().to_string());
    assert_eq!(params["mode"], "auto");
    let blob = variants::portable::base64_decode(params["ir_b64"].as_str().unwrap()).unwrap();
    assert_eq!(blob, ir.encode());
    let (params, _) = reload_params(rules::builtin::SLAYER, ReloadMode::InPlace).unwrap();
    assert_eq!(params["mode"], "in_place");

    let mut ed = RulesEditor { text: "rules".into(), ..Default::default() };
    let why = ed.queue_reload().unwrap_err();
    assert!(why.contains("does not build"), "{why}");
    assert!(ed.outbox.is_none());
    assert_eq!(ed.reload_line().unwrap().0, Tone::Bad);
}

#[test]
fn reload_rows_say_how_it_went() {
    assert_eq!(reload_line(&ReloadState::Idle), None);
    let done = |mode, reasons: Vec<String>| {
        ReloadState::Done(ReloadResult { mode, hash: "0123456789abcdef".repeat(4), reasons, tick: 90 })
    };
    let (tone, text) = reload_line(&done(Reloaded::InPlace, vec![])).unwrap();
    assert_eq!(tone, Tone::Good);
    assert_eq!(text, "Script 0123456789ab: reloaded in place at tick 90");
    let (tone, text) =
        reload_line(&done(Reloaded::Restarted, vec!["player variable best changed".into()])).unwrap();
    assert_eq!(tone, Tone::Notice);
    assert!(text.ends_with("restarted with it (player variable best changed)"), "{text}");
    let (tone, _) = reload_line(&ReloadState::Sending("ab".into())).unwrap();
    assert_eq!(tone, Tone::Busy);
}

#[test]
fn starters_open_by_name_and_files_save_and_open() {
    let mut ed = RulesEditor::default();
    ed.open_starter("slayer").unwrap();
    assert_eq!(ed.text, rules::builtin::SLAYER);
    assert_eq!(ed.title, "Slayer (built in)");
    assert!(ed.path.is_none() && ed.dirty());
    ed.open_starter("Infection (example)").unwrap();
    assert!(ed.text.contains("Infection"));
    assert!(ed.open_starter("no such").unwrap_err().contains("Slayer (built in)"));

    let dir = std::env::temp_dir().join(format!("h2-editor-rules-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mine.rules");
    assert!(ed.save(None).is_err(), "no file yet");
    ed.save(Some(&path)).unwrap();
    assert!(!ed.dirty());
    assert_eq!(ed.title, "mine.rules");
    let mut other = RulesEditor::default();
    other.open_path(&path).unwrap();
    assert_eq!(other.text, ed.text);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_analysis_follows_the_text() {
    let mut ed = RulesEditor::default();
    assert!(ed.refresh().check.as_ref().unwrap().ok, "the blank script checks");
    ed.text.push_str("\nnonsense here\n");
    assert!(!ed.refresh().diagnostics.is_empty());
    assert!(!ed.refresh().check.as_ref().unwrap().ok);
}
