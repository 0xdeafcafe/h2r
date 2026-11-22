//! The script editor's pure helpers: positions, colour spans, problem
//! rows, completion text and the reload's params and row; and the
//! language features over `rules::ide` (completion as you type, quick
//! fixes, rename, go to definition, uses, hover).

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
    // The word is replaced, the first placeholder selected, and the widget
    // gets the same edit.
    assert_eq!(ed.text, "    score(p, n)");
    let edit = ed.edit.clone().expect("an edit for the widget");
    assert_eq!(edit.text, ed.text);
    assert_eq!(edit.select, Some((10, 11)));
    assert_eq!(&ed.text[10..11], "p");
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

/// The bug behind "can't see the text" was in the drawing (see
/// `headless::the_script_editor_shows_its_text`); the state half: an
/// open fills the buffer the widget draws and hands the widget the same
/// text while it has the keyboard.
#[test]
fn opening_a_script_fills_the_buffer_and_hands_it_to_the_widget() {
    let mut ed = RulesEditor::default();
    assert_eq!(ed.text, BLANK);
    ed.open_starter("slayer").unwrap();
    assert_eq!(ed.text, rules::builtin::SLAYER);
    assert_eq!(ed.edit, Some(Edit::to(rules::builtin::SLAYER.to_string(), 0)));
    assert!(ed.show, "an open picks the window's tab");
    assert!(ed.menu.is_none());

    // `rules.set` with the widget idle: the text, and no keyboard grab.
    let mut ed = RulesEditor::default();
    ed.set_text("rules \"A\" 1\n".into());
    assert_eq!(ed.text, "rules \"A\" 1\n");
    assert!(ed.edit.is_none());
    // While typing: the widget takes it.
    ed.typing = true;
    ed.set_text("rules \"B\" 1\n".into());
    assert_eq!(ed.edit.as_ref().map(|e| e.text.as_str()), Some("rules \"B\" 1\n"));
}

#[test]
fn snippets_expand_to_their_defaults_with_the_first_selected() {
    let e = expand_snippet("score(${1:player}, ${2:int})", "");
    assert_eq!(e.text, "score(player, int)");
    assert_eq!(e.select, Some((6, 12)));
    assert_eq!(e.cursor, 12);

    // New lines take the line's indent, tabs become spaces, and $0 is the
    // cursor when no placeholder comes first.
    let e = expand_snippet("kill(${1:victim}, ${2:killer}) {\n\t$0\n}", "    ");
    assert_eq!(e.text, "kill(victim, killer) {\n        \n    }");
    assert_eq!(e.select, Some((5, 11)));
    let e = expand_snippet("a $0b", "");
    assert_eq!((e.text.as_str(), e.cursor, e.select), ("a b", 2, None));

    // Choices give their first; nesting and escapes read through.
    assert_eq!(expand_snippet("list ${1|research,party|}", "").text, "list research");
    assert_eq!(expand_snippet("carrier ${1|slayer,ctf|}", "").text, "carrier slayer");
    let e = expand_snippet("${1:outer ${2:inner}}", "");
    assert_eq!((e.text.as_str(), e.select), ("outer inner", Some((0, 11))));
    assert_eq!(expand_snippet(r"\$x", "").text, "$x");
    let e = expand_snippet("plain", "");
    assert_eq!((e.cursor, e.select), (5, None));
}

fn item(label: &str, sort: &str) -> ide::CompletionItem {
    ide::CompletionItem {
        label: label.into(),
        kind: ide::kind::FUNCTION,
        detail: String::new(),
        documentation: ide::Markup::markdown(""),
        insert_text: label.into(),
        insert_text_format: ide::PLAIN_TEXT,
        sort_text: sort.into(),
    }
}

#[test]
fn completions_rank_by_the_word_then_the_languages_order() {
    let items = [item("score", "0002"), item("player", "0000"), item("set_score", "0001"), item("kills", "0003")];
    let labels = |word: &str| -> Vec<String> { rank(&items, word).iter().map(|i| i.label.clone()).collect() };
    assert_eq!(labels(""), ["player", "set_score", "score", "kills"]);
    // Starts with, then holds, then has its letters in order; case ignored.
    assert_eq!(labels("sc"), ["score", "set_score"]);
    assert_eq!(labels("SC"), ["score", "set_score"]);
    assert_eq!(labels("plr"), ["player"]);
    assert!(labels("zzz").is_empty());
}

#[test]
fn typing_opens_narrows_takes_and_closes_the_completions() {
    let text = "rules \"T\" 1\ncarrier slayer\non kill(victim, killer) {\n    sco\n}\n";
    let at = text.find("sco").unwrap() + 3;
    let mut ed = RulesEditor { text: text.into(), cursor: at, ..Default::default() };
    ed.after_typing(Some('o'));
    let menu = ed.menu.clone().expect("typing a name opens the list");
    assert_eq!(menu.kind, MenuKind::Completions);
    assert!(menu.choices[0].label.to_lowercase().starts_with("sco"), "{:?}", menu.choices[0]);
    let i = menu.choices.iter().position(|c| c.label == "score").expect("score is offered");

    // Up and Down move the selection, wrapping.
    ed.menu_move(-1);
    assert_eq!(ed.menu.as_ref().unwrap().selected, menu.choices.len() - 1);
    ed.menu_move(1);
    assert_eq!(ed.menu.as_ref().unwrap().selected, 0);
    assert_eq!(ed.menu_edit(), Some(menu.choices[0].edit.clone()));

    // Taking one: the snippet's defaults, the first selected, the list shut.
    ed.choose(i);
    assert!(ed.text.contains("    score(player, int)\n"), "{}", ed.text);
    let edit = ed.edit.clone().unwrap();
    let (a, b) = edit.select.unwrap();
    assert_eq!(&ed.text[a..b], "player");
    assert!(ed.menu.is_none());

    // A space closes the list; so does a deletion that empties the word.
    let mut ed = RulesEditor { text: text.into(), cursor: at, ..Default::default() };
    ed.after_typing(Some('o'));
    ed.after_typing(Some(' '));
    assert!(ed.menu.is_none());
    ed.after_typing(Some('o'));
    assert!(ed.menu.is_some());
    ed.text = text.replace("sco", "");
    ed.cursor = at - 3;
    ed.after_typing(None);
    assert!(ed.menu.is_none(), "opened by typing, nothing typed now: closed");

    // Ctrl+Space shows the list with nothing typed.
    ed.open_completions(true);
    let menu = ed.menu.as_ref().expect("a list with nothing typed");
    assert!(menu.manual && !menu.choices.is_empty());

    assert_eq!(trigger('a'), Trigger::Open);
    assert_eq!(trigger('_'), Trigger::Open);
    assert_eq!(trigger('.'), Trigger::Open);
    assert_eq!(trigger(' '), Trigger::Close);
    assert_eq!(trigger('('), Trigger::Close);
}

#[test]
fn text_edits_apply_together_and_carry_the_cursor() {
    let text = "abc def ghi";
    let edit = |a: u32, b: u32, new: &str| TextEdit {
        range: Range { start: Position::new(0, a), end: Position::new(0, b) },
        new_text: new.into(),
    };
    let edits = [edit(4, 7, "yyyy"), edit(0, 3, "x")];
    let out = apply_text_edits(text, 8, &edits);
    assert_eq!(out.text, "x yyyy ghi");
    assert_eq!(&out.text[out.cursor..], "ghi", "edits before the cursor shift it");
    let out = apply_text_edits(text, 5, &edits);
    assert_eq!(out.cursor, "x yyyy".len(), "an edit around the cursor puts it at the edit's end");
}

#[test]
fn quick_fixes_at_the_cursor_and_on_the_problem_rows() {
    let src = "rules \"T\" 1\ncarrier slayer\nlabel barrier: objects\non tick { for each barier b { } }\n";
    let mut ed = RulesEditor { text: src.into(), cursor: src.find("barier").unwrap() + 2, ..Default::default() };
    ed.open_fixes().unwrap();
    let menu = ed.menu.clone().unwrap();
    assert_eq!(menu.kind, MenuKind::Fixes);
    let titles: Vec<&str> = menu.choices.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(titles, ["Change to `barrier`", "Declare label `barier`"]);
    ed.choose(0);
    assert!(ed.text.contains("for each barrier b"), "{}", ed.text);
    assert!(ed.edit.is_some() && ed.menu.is_none());

    // Nothing to fix on the header.
    let mut ed = RulesEditor { text: src.into(), cursor: 1, ..Default::default() };
    assert!(ed.open_fixes().is_err());

    // The same fixes on the problem's row.
    let rows = problems(src, &ide::diagnostics(src));
    let row = rows.iter().find(|r| r.code == "label.unknown").unwrap();
    assert_eq!(row.fixes.len(), 2);
    ed.apply_fix(&row.fixes[0]);
    assert!(ed.text.contains("for each barrier b"), "{}", ed.text);
}

#[test]
fn rename_definition_and_uses_follow_the_name() {
    let decl = BLANK.find("kill_points").unwrap();
    let used = BLANK.rfind("kill_points").unwrap();
    assert_ne!(decl, used);
    assert_eq!(definition_at(BLANK, used + 3), Some(decl));
    assert_eq!(word_at(BLANK, used + 3), Some((used, used + "kill_points".len())));
    assert_eq!(word_at(BLANK, BLANK.find("1..5").unwrap()), None, "a number is no name");

    let marks = occurrences(BLANK, used + 3);
    assert_eq!(marks.len(), 2, "{marks:?}");
    for m in &marks {
        let line = BLANK.split('\n').nth(m.line).unwrap();
        assert_eq!(&line[m.start..m.end], "kill_points");
    }

    let mut ed = RulesEditor { cursor: used + 3, ..Default::default() };
    ed.go_to_definition(ed.cursor).unwrap();
    assert_eq!(ed.want_cursor, Some(decl));

    ed.cursor = used + 3;
    ed.start_rename().unwrap();
    assert_eq!(ed.renaming.as_ref().map(|r| r.old.as_str()), Some("kill_points"));
    ed.renaming.as_mut().unwrap().name = "points".into();
    ed.finish_rename().unwrap();
    assert!(!ed.text.contains("kill_points"), "{}", ed.text);
    assert!(ed.text.contains("option points:") && ed.text.contains("score(killer, points)"), "{}", ed.text);
    assert!(ed.renaming.is_none() && ed.edit.is_some());

    // A keyword is refused, and the field stays open.
    ed.cursor = ed.text.find("points").unwrap();
    ed.start_rename().unwrap();
    ed.renaming.as_mut().unwrap().name = "if".into();
    assert!(ed.finish_rename().is_err());
    assert!(ed.renaming.is_some());
    assert!(rename_edit(BLANK, used, "if").is_err());
}

#[test]
fn hovers_read_as_code_and_prose_and_find_their_problems() {
    let blocks = hover_blocks("```rules\nscore(player, int)\n```\nAdds **points** to `p`.");
    assert_eq!(
        blocks,
        [
            HoverBlock { code: true, text: "score(player, int)".into() },
            HoverBlock { code: false, text: "Adds points to p.".into() },
        ]
    );
    let text = "rules \"x\" 1\ncarrier slayer\n\non kill(victim, killer) {\n    score(victim, nope)\n}\n";
    let diags = ide::diagnostics(text);
    let d = diags.iter().find(|d| d.message.contains("nope")).unwrap();
    assert!(diagnostics_at(&diags, d.range.start).contains(&d));
    assert!(diagnostics_at(&diags, Position::new(0, 0)).is_empty());
}

#[test]
fn format_hands_the_widget_the_laid_out_text() {
    let src = "rules \"T\" 1\ncarrier slayer\non kill(victim, killer) {\nif killer != none {\nscore(killer, 1)\n}\n}\n";
    let want = ide::format(src).unwrap();
    let mut ed = RulesEditor { text: src.into(), ..Default::default() };
    ed.format().unwrap();
    assert_eq!(ed.text, want);
    assert_eq!(ed.edit.is_some(), want != src);
}

#[test]
fn open_in_editor_names_the_file_after_the_script() {
    assert_eq!(script_file_name("Slayer (built in)"), "slayer.rules");
    assert_eq!(script_file_name("King of the Hill (example)"), "king_of_the_hill.rules");
    assert_eq!(script_file_name("mine.rules"), "mine.rules");
    assert_eq!(script_file_name("Untitled"), "untitled.rules");
    assert_eq!(script_file_name("a/b: c?"), "ab_c.rules");
    assert_eq!(script_file_name(""), "script.rules");

    let root = Path::new("/data");
    let own = Path::new("/work/x.rules");
    assert_eq!(external_path(Some(own), "Slayer (built in)", root, |_| true).as_path(), own, "its own file wins");
    assert_eq!(
        external_path(None, "Slayer (built in)", root, |_| false).as_path(),
        Path::new("/data/scripts/slayer.rules")
    );
    let taken = [PathBuf::from("/data/scripts/slayer.rules"), PathBuf::from("/data/scripts/slayer-2.rules")];
    assert_eq!(
        external_path(None, "Slayer (built in)", root, |p| taken.iter().any(|t| t == p)).as_path(),
        Path::new("/data/scripts/slayer-3.rules"),
        "a name in use gets a number"
    );
}

#[test]
fn open_in_editor_tries_an_editor_with_the_extension_then_the_users_editors() {
    let file = Path::new("/s/x.rules");
    let bin = |name: &str| Some(PathBuf::from(format!("/bin/{name}")));
    let env = |k: &str| match k {
        "VISUAL" => Some("subl -w".to_string()),
        "EDITOR" => Some("vim".to_string()),
        _ => None,
    };
    let describe = |got: Vec<Launch>| -> Vec<String> { got.iter().map(Launch::describe).collect() };

    // Zed has the extension: it comes first, and once.
    let got = editor_launches(file, env, bin, |e| e == "zed", true);
    assert_eq!(
        describe(got),
        [
            "/bin/zed /s/x.rules",
            "subl -w /s/x.rules",
            // vim needs a terminal: passed over.
            "/bin/code /s/x.rules",
            "open -t /s/x.rules",
        ]
    );

    // Neither has it: $VISUAL first, then VS Code and Zed as they are.
    let got = editor_launches(file, env, bin, |_| false, true);
    assert_eq!(
        describe(got),
        ["subl -w /s/x.rules", "/bin/code /s/x.rules", "/bin/zed /s/x.rules", "open -t /s/x.rules"]
    );

    // An extension in an editor that is not found counts for nothing;
    // nothing found and not macOS: nothing to try.
    let got = editor_launches(file, |_| None, |_| None, |_| true, false);
    assert!(got.is_empty());

    // $VISUAL and $EDITOR the same, and VS Code with the extension: tried
    // once each.
    let env = |k: &str| matches!(k, "VISUAL" | "EDITOR").then(|| "code --wait".to_string());
    let got = editor_launches(file, env, bin, |e| e == "code", true);
    assert_eq!(
        describe(got),
        ["/bin/code /s/x.rules", "code --wait /s/x.rules", "/bin/zed /s/x.rules", "open -t /s/x.rules"]
    );

    // Where each editor keeps the extension.
    let home = Path::new("/home/me");
    let vscode = |dir: &Path| -> Vec<String> {
        if dir == Path::new("/home/me/.vscode/extensions") {
            vec!["rust-lang.rust-analyzer-0.3.1".into(), "h2recomp.h2-rules-0.1.0".into()]
        } else {
            Vec::new()
        }
    };
    assert!(extension_installed("code", home, true, |_| false, vscode));
    assert!(!extension_installed("code", home, true, |_| false, |_| Vec::new()));
    let zed_mac = |p: &Path| p == Path::new("/home/me/Library/Application Support/Zed/extensions/installed/h2-rules");
    assert!(extension_installed("zed", home, true, zed_mac, |_| Vec::new()));
    assert!(!extension_installed("zed", home, false, zed_mac, |_| Vec::new()));
    let zed_linux = |p: &Path| p == Path::new("/home/me/.local/share/zed/extensions/installed/h2-rules");
    assert!(extension_installed("zed", home, false, zed_linux, |_| Vec::new()));
    assert!(!extension_installed("subl", home, true, |_| true, vscode));

    assert!(is_terminal_editor("/usr/bin/nvim") && is_terminal_editor("nano"));
    assert!(!is_terminal_editor("code") && !is_terminal_editor("/usr/local/bin/zed"));
    let dirs = program_dirs(Some("/a:/usr/local/bin::/b"));
    assert_eq!(dirs, ["/a", "/usr/local/bin", "/b", "/opt/homebrew/bin"].map(PathBuf::from));
    assert_eq!(find_program("zed", &dirs, |p| p == Path::new("/b/zed")), Some(PathBuf::from("/b/zed")));
}

#[test]
fn a_started_editor_counts_as_open_once_it_succeeds_or_keeps_running() {
    let early = Duration::from_millis(200);
    assert_eq!(launch_step(None, early), LaunchStep::Wait);
    assert_eq!(launch_step(None, LAUNCH_GRACE), LaunchStep::Done, "a GUI editor still running");
    assert_eq!(launch_step(Some(true), early), LaunchStep::Done, "code and open exit at once");
    assert_eq!(launch_step(Some(false), early), LaunchStep::Next, "an editor that failed to start");
}

#[test]
fn an_outside_save_comes_back_or_asks_when_both_changed() {
    assert_eq!(disk_change("a", "a", "a"), DiskChange::Same);
    assert_eq!(disk_change("b", "a", "b"), DiskChange::Same, "the file caught up with the buffer");
    assert_eq!(disk_change("a", "a", "c"), DiskChange::Take);
    assert_eq!(disk_change("b", "a", "c"), DiskChange::Ask);

    // No unsaved edits: the buffer takes the file's text, saved.
    let mut ed = RulesEditor::default();
    let theirs = BLANK.replace("New game type", "Theirs");
    assert_eq!(ed.disk_text(theirs.clone()), DiskPoll::Took);
    assert_eq!(ed.text, theirs);
    assert!(!ed.dirty() && ed.conflict.is_none());

    // Unsaved edits here: asked, nothing lost until answered.
    let mine = BLANK.replace("New game type", "Mine");
    ed.text = mine.clone();
    let newer = BLANK.replace("New game type", "Newer");
    assert_eq!(ed.disk_text(newer.clone()), DiskPoll::Asked);
    assert_eq!(ed.text, mine);
    assert_eq!(ed.conflict.as_deref(), Some(newer.as_str()));
    ed.keep_mine();
    assert!(ed.conflict.is_none() && ed.text == mine && ed.dirty(), "kept, still unsaved");

    assert_eq!(ed.disk_text(newer.clone()), DiskPoll::Asked);
    assert!(ed.take_conflict());
    assert_eq!(ed.text, newer);
    assert!(!ed.dirty() && ed.conflict.is_none());
    assert!(!ed.take_conflict(), "nothing left to take");

    // The file catching up with the buffer just marks it saved.
    ed.text = mine.clone();
    assert_eq!(ed.disk_text(mine.clone()), DiskPoll::Nothing);
    assert!(!ed.dirty());
}

#[test]
fn the_watch_checks_once_a_second_and_sees_new_times() {
    let t0 = Instant::now();
    let then = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
    let later = then + Duration::from_secs(5);
    let mut w = Watch::new(PathBuf::from("/x.rules"), Some(then));
    assert!(w.due(t0));
    assert!(!w.due(t0 + Duration::from_millis(500)));
    assert!(w.due(t0 + WATCH_EVERY));
    assert!(!w.changed(Some(then)));
    assert!(w.changed(Some(later)));
    assert!(!w.changed(Some(later)), "seen once");
    assert!(!w.changed(None), "gone mid-save is no change");
    assert!(w.changed(Some(later)), "back again is one");
}

#[test]
fn open_in_editor_writes_the_script_and_watches_its_file() {
    let root = std::env::temp_dir().join(format!("h2-editor-rules-ext-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut ed = RulesEditor::default();
    ed.open_starter("slayer").unwrap();
    let path = ed.prepare_external(&root).unwrap();
    assert_eq!(path, root.join("scripts").join("slayer.rules"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), rules::builtin::SLAYER);
    assert_eq!(ed.path.as_deref(), Some(path.as_path()));
    assert!(!ed.dirty());
    assert_eq!(ed.watch.as_ref().map(|w| w.path.clone()), Some(path.clone()));
    assert_eq!(ed.poll_disk(Instant::now()), DiskPoll::Nothing, "unchanged");

    // The outside editor saves: the next due poll takes it.
    let theirs = rules::builtin::SLAYER.replace("Slayer", "Theirs");
    std::fs::write(&path, &theirs).unwrap();
    if let Some(w) = &mut ed.watch {
        w.seen = Some(SystemTime::UNIX_EPOCH);
        w.next = None;
    }
    assert_eq!(ed.poll_disk(Instant::now()), DiskPoll::Took);
    assert_eq!(ed.text, theirs);

    // A second open keeps the same file; opening another script stops
    // the watch.
    assert_eq!(ed.prepare_external(&root).unwrap(), path);
    ed.open_starter("slayer").unwrap();
    assert!(ed.watch.is_none());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn the_script_section_lists_marks() {
    // A script that marks settings: one line per mark, in its order, and
    // one per setting the block names.
    let mut ed = RulesEditor::default();
    ed.open_starter("SWAT").unwrap();
    let check = ed.refresh().check.clone().unwrap();
    assert!(check.ok, "{:?}", check.errors);
    let rep = check.report.as_ref().unwrap();
    let section = script_section(&check).unwrap();
    assert_eq!(section.settings.len(), rep.settings.len());
    assert!(!section.settings.is_empty());
    assert_eq!(section.quick.len(), rep.quick.len());
    assert_eq!(section.quick.len(), 5, "swat's quick line has five names");
    for (i, (line, q)) in section.quick.iter().zip(&rep.quick).enumerate() {
        assert!(line.starts_with(&format!("{}. {}", i + 1, q.label)), "{line}");
        assert!(line.ends_with("(setting)"), "{line}");
    }
    for (line, s) in section.settings.iter().zip(&rep.settings) {
        assert_eq!(line, &format!("{}: {}", s.label, s.text));
    }

    // Options show as such, in the script's order among the settings.
    ed.open_starter("Superbounce (built in)").unwrap();
    let check = ed.refresh().check.clone().unwrap();
    let section = script_section(&check).unwrap();
    let kinds: Vec<&str> = section
        .quick
        .iter()
        .map(|l| if l.ends_with("(option)") { "option" } else { "setting" })
        .collect();
    assert_eq!(kinds, ["setting", "setting", "setting", "option", "option"]);

    // A script that names none has no section.
    ed.apply(Edit::to(BLANK.to_string(), 0));
    let check = ed.refresh().check.clone().unwrap();
    assert_eq!(script_section(&check), None);
}

#[test]
fn a_script_saves_as_its_own_game_variant() {
    let src = rules::builtin::SWAT;
    let (name, text) = game_variant_file(src).unwrap();
    assert_eq!(name, "SWAT.h2variant");
    // The file is what `from_script` makes, source attached.
    let ir = rules::compile(src).unwrap();
    let want = variants::portable::from_script(&ir, Some(src), &[]).unwrap();
    let got = variants::portable::read(&text).unwrap();
    assert_eq!(got.id(), want.id());
    assert_eq!(got.name, "SWAT");

    let ed = RulesEditor { text: src.to_string(), ..Default::default() };
    let dir = std::env::temp_dir().join(format!("h2-script-variant-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = ed.save_game_variant(&dir.join(&name)).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    std::fs::remove_dir_all(&dir).unwrap();

    // A script that does not build writes nothing.
    let bad = RulesEditor { text: "rules".into(), ..Default::default() };
    assert!(bad.save_game_variant(&dir.join("x.h2variant")).is_err());
    assert!(!dir.exists());
}

#[test]
fn a_script_is_found_by_its_hash_prefix() {
    let ed = RulesEditor::default();
    let ir = rules::compile(rules::builtin::SWAT).unwrap();
    let prefix: [u8; 8] = ir.id().0[..8].try_into().unwrap();
    assert_eq!(find_script(prefix, &ed).unwrap().id(), ir.id());
    assert!(find_script([9; 8], &ed).is_none());
    assert_eq!(hex_prefix([0xab, 1, 2, 3, 4, 5, 6, 7]), "ab01020304050607");

    // The script open in the editor is found too, once analysed.
    let mut ed = RulesEditor::default();
    ed.refresh();
    let own = rules::compile(BLANK).unwrap();
    let prefix: [u8; 8] = own.id().0[..8].try_into().unwrap();
    assert_eq!(find_script(prefix, &ed).unwrap().id(), own.id());
}
