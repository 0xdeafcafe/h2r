use super::*;
use hud_kit_core::views::forge::{Hand, Tool, prompt_text};

fn view(tool: Tool, hand: Hand, target: &str) -> View {
    View {
        monitor: true,
        tool,
        hand,
        target: target.into(),
        used: 4,
        limit: 20,
        ..View::default()
    }
}

fn published(view: View) -> Published {
    Published { view, notice: None }
}

/// The prompts name the controls the table binds: the preview's sample is
/// Halo 3's layout, and so is the table.
#[test]
fn the_buttons_are_read_from_the_table() {
    assert_eq!(buttons(), Buttons::sample());
    assert_eq!(buttons().accept.press(), "Press \u{e100}");
    assert_eq!(buttons().objects.press(), "Press \u{e102}");
    assert_eq!(buttons().delete.press(), "Hold \u{e103}");
    assert_eq!(buttons().turn.press(), "Hold \u{e107}");
    assert_eq!(buttons().menu.press(), "Press \u{e10c}");
}

/// Every pad button has its own glyph in the fonts' button range, and every
/// row of the table a cue.
#[test]
fn every_button_has_a_glyph_and_every_row_a_cue() {
    let mut seen = BTreeSet::new();
    for b in HostButton::ALL {
        let g = glyph(b);
        assert!(('\u{e100}'..='\u{e111}').contains(&g), "{b:?}");
        assert!(seen.insert(g), "{b:?} shares a glyph");
    }
    for row in bindings::BINDINGS {
        let c = cue(row.does);
        assert!(('\u{e100}'..='\u{e111}').contains(&c.glyph), "{}", row.name);
        assert_eq!(
            c.hold,
            matches!(row.pad, Pad::Hold(_) | Pad::Trigger(_)),
            "{}",
            row.name
        );
    }
}

/// A prompt written from the table says the button the table binds, in
/// every state of both tools.
#[test]
fn every_tool_state_prompts_with_a_table_button() {
    let b = buttons();
    for tool in Tool::ALL {
        for &hand in tool.hands() {
            let text = prompt_text(&view(tool, hand, "Beacon"), &b);
            assert!(!text.is_empty(), "{tool:?} {hand:?}");
            if hand == Hand::Idle && tool == Tool::Objects {
                assert!(text.contains(b.objects.glyph), "{text}");
            }
            if hand != Hand::Idle && tool == Tool::Objects {
                assert!(text.contains(b.accept.glyph), "{text}");
            }
        }
    }
}

/// The monitor off hides everything, sends no notice and teaches nothing.
#[test]
fn nothing_is_published_with_the_monitor_off() {
    let mut teaching = Teaching::default();
    let mut sent = None;
    let p = Published {
        view: View::default(),
        notice: Some(("Saved Courtyard.".into(), 5)),
    };
    let (values, notice) = frame(&p, &mut teaching, &mut sent, &buttons());
    assert!(values.0.iter().all(|(_, v)| !v.shown));
    assert_eq!(notice, None);
    assert_eq!(sent, None);
    assert_eq!(values.layout_problems(&kit::hud()), Vec::<String>::new());
}

/// A notice is sent once; the same line at a later tick is sent again, and
/// one that is gone lets the next through.
#[test]
fn a_notice_is_sent_once_each() {
    let mut teaching = Teaching::default();
    let mut sent = None;
    let b = buttons();
    let mut p = published(view(Tool::Objects, Hand::Idle, ""));
    assert_eq!(frame(&p, &mut teaching, &mut sent, &b).1, None);
    p.notice = Some(("The object budget is full.".into(), 10));
    assert_eq!(
        frame(&p, &mut teaching, &mut sent, &b).1.as_deref(),
        Some("The object budget is full.")
    );
    assert_eq!(
        frame(&p, &mut teaching, &mut sent, &b).1,
        None,
        "same stamp"
    );
    p.notice = Some(("The object budget is full.".into(), 40));
    assert!(
        frame(&p, &mut teaching, &mut sent, &b).1.is_some(),
        "said again"
    );
    p.notice = None;
    assert_eq!(frame(&p, &mut teaching, &mut sent, &b).1, None);
    assert_eq!(sent, None);
}

/// The notification list keeps a line whole: a long sentence sends its
/// first sentence, and one with no short first sentence none.
#[test]
fn a_notice_line_is_whole_or_its_first_sentence() {
    assert_eq!(
        notice_line("Saved Courtyard.").as_deref(),
        Some("Saved Courtyard.")
    );
    assert_eq!(notice_line("  "), None);
    let long =
        "Forge with players on other consoles is not available. Leave the party to edit alone.";
    assert_eq!(
        notice_line(long).as_deref(),
        Some("Forge with players on other consoles is not available.")
    );
    let none =
        "A sentence with no stop that goes on and on past what the list keeps whole in one line";
    assert_eq!(notice_line(none), None);
    let wide = format!("{} and more. Short.", "word ".repeat(20));
    assert_eq!(
        notice_line(&wide),
        None,
        "the first sentence is too long too"
    );
}

/// A hint shows the first time its state is reached, for its time, and not
/// again; one left within a second shows again.
#[test]
fn a_hint_is_shown_once_for_its_time() {
    let mut t = Teaching::default();
    assert_eq!(t.step(None), None);
    // Reached: shown for TEACH_TICKS ticks.
    for _ in 0..TEACH_TICKS {
        assert_eq!(t.step(Some(Teach::Delete)), Some(Teach::Delete));
    }
    assert_eq!(t.step(Some(Teach::Delete)), None, "taught");
    assert_eq!(t.step(Some(Teach::Delete)), None);
    assert_eq!(t.step(None), None);
    assert_eq!(t.step(Some(Teach::Delete)), None, "and not again");
    // Another hint has its own turn.
    assert_eq!(t.step(Some(Teach::Turn)), Some(Teach::Turn));
    // Left at once, it was not read: shown again.
    assert_eq!(t.step(None), None);
    assert_eq!(t.step(Some(Teach::Turn)), Some(Teach::Turn));
    // Left after a second, it is taught.
    for _ in 0..TEACH_MIN {
        assert_eq!(t.step(Some(Teach::Turn)), Some(Teach::Turn));
    }
    assert_eq!(t.step(None), None);
    assert_eq!(t.step(Some(Teach::Turn)), None);
}

/// The hint rides in the training box while its state lasts.
#[test]
fn the_hint_is_in_the_training_box() {
    let mut teaching = Teaching::default();
    let mut sent = None;
    let b = buttons();
    let p = published(view(Tool::Objects, Hand::Aimed, "Beacon"));
    let (values, _) = frame(&p, &mut teaching, &mut sent, &b);
    let text = &values.get(kit::TEACH).unwrap().text;
    assert!(values.get(kit::TEACH).unwrap().shown);
    assert!(text.contains(b.delete.glyph), "{text}");
    // The Links tool teaches nothing.
    let mut teaching = Teaching::default();
    let p = published(view(Tool::Links, Hand::Aimed, "Gate"));
    let (values, _) = frame(&p, &mut teaching, &mut sent, &b);
    assert!(!values.get(kit::TEACH).unwrap().shown);
}
