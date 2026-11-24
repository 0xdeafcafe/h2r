use super::*;
use crate::forge_screens::sheet::{self, Act, Ask, Choice};
use forge::files::variants::Problem;

fn listed(file: &str, source: Source, problem: Option<Problem>) -> Listed {
    Listed {
        source,
        file: file.into(),
        name: file.to_uppercase(),
        author: String::new(),
        id: None,
        problem,
        sentence: problem.map(|p| p.sentence(file, "fixture")),
    }
}

fn write(name: &str, replace: bool) -> SaveStep {
    SaveStep::Write {
        name: name.into(),
        replace,
    }
}

/// The table of flows: what each press does, and which notice or question
/// it ends in.
#[test]
fn save_flows() {
    // Save: an unnamed session asks for a name; a named one writes over its own.
    assert_eq!(save(None), SaveStep::Keyboard);
    assert_eq!(save(Some("Arena")), write("Arena", true));
    // Save as: the typed name, trimmed.
    assert_eq!(save_as(None, "  Arena "), Ok(write("Arena", false)));
    // The open variant's own name, with any case, replaces without asking.
    assert_eq!(save_as(Some("Arena"), "ARENA"), Ok(write("ARENA", true)));
    // Another name does not: the store says it is taken and the question follows.
    assert_eq!(save_as(Some("Arena"), "Keep"), Ok(write("Keep", false)));
    // A name the store would refuse never reaches it.
    assert!(save_as(None, "   ").is_err());
    assert!(save_as(None, &"x".repeat(forge_doc::MAX_VARIANT_NAME + 1)).is_err());
    assert!(save_as(None, &"x".repeat(forge_doc::MAX_VARIANT_NAME)).is_ok());
}

#[test]
fn the_notices_say_what_happened() {
    assert_eq!(saved("Arena"), "Saved Arena.");
    assert_eq!(loaded("Arena"), "Loaded Arena.");
    assert_eq!(replace_question("Arena"), "Replace Arena?");
}

#[test]
fn load_offers_what_this_machine_saved_and_can_use() {
    let rows = rows(&[
        listed("a", Source::Saved, None),
        listed("b", Source::Saved, Some(Problem::BaseHash)),
        listed("c", Source::Live, None),
    ]);
    assert_eq!(rows.len(), 3);
    let offered: Vec<&str> = loadable(&rows).iter().map(|r| r.file.as_str()).collect();
    assert_eq!(offered, ["a"]);
    assert!(
        rows[1]
            .sentence
            .as_deref()
            .is_some_and(|s| s.contains("another version"))
    );
}

#[test]
fn the_questions_ask_in_the_specs_words_and_focus_no() {
    for (ask, name, body) in [
        (Ask::Overwrite, "Arena", "Replace Arena?"),
        (Ask::Load, "", "Load without saving?"),
    ] {
        let s = sheet::question(ask, name);
        assert_eq!(s.body, body);
        assert_eq!(s.rows[s.focus].label, "No");
        assert_eq!(
            s.rows[s.focus].act,
            Act::Choose(Choice::Answer { ask, yes: false })
        );
    }
}
