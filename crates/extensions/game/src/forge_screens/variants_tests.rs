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

fn row(file: &str, name: &str, saved: bool, problem: Option<Problem>) -> Row {
    Row::of(&listed(
        file,
        if saved { Source::Saved } else { Source::Live },
        problem,
    ))
    .named(name)
}

impl Row {
    fn named(mut self, name: &str) -> Row {
        self.name = name.into();
        self
    }
}

fn labels(sheet: &Sheet) -> Vec<&str> {
    sheet.rows.iter().map(|r| r.label.as_str()).collect()
}

#[test]
fn the_page_says_where_each_variant_is_or_why_it_cannot_be_used() {
    let rows = vec![
        row("a", "Arena", true, None),
        row("b", "Tower", false, None),
        row("c", "Old", true, Some(Problem::BaseHash)),
    ];
    let page = list_sheet(&rows, Some("arena"));
    assert_eq!(page.kind, Kind::Variants);
    assert_eq!(labels(&page), ["Arena", "Tower", "Old"]);
    let values: Vec<_> = page.rows.iter().map(|r| r.value.as_deref()).collect();
    assert_eq!(
        values,
        [Some(SAVED), Some(ON_LIVE), Some(UNUSABLE)]
    );
    assert_eq!(
        page.rows[2].description.as_deref(),
        rows[2].sentence.as_deref()
    );
    assert!(matches!(&page.rows[0].act, Act::Variant(file, _) if file == "a"));
    // A map with none has one fact row.
    let empty = list_sheet(&[], None);
    assert_eq!(labels(&empty), [NO_VARIANTS]);
    assert_eq!(empty.rows[0].act, Act::Info);
}

#[test]
fn a_dialog_has_the_rows_the_variant_can_act_on() {
    let saved = row("a", "Arena", true, None);
    assert_eq!(
        labels(&variant_sheet(&saved, Some("Other"))),
        ["Load", "Rename", "Duplicate", "Delete"]
    );
    // The open variant's Delete is absent, and its row says why.
    let open = variant_sheet(&saved, Some("ARENA"));
    assert_eq!(labels(&open), ["Load", "Rename", "Duplicate", "Open now"]);
    assert_eq!(
        open.rows[3].description.as_deref(),
        Some(OPEN_CANNOT_DELETE)
    );
    assert!(open.rows.iter().all(|r| r.act != Act::Choose(Choice::DeleteVariant("a".into()))));
    // One that fails a check is renamed and deleted, not loaded.
    let bad = row("c", "Old", true, Some(Problem::BaseHash));
    assert_eq!(labels(&variant_sheet(&bad, None)), ["Rename", "Delete"]);
    // A Live copy is not renamed or copied here.
    let live = row("b", "Tower", false, None);
    assert_eq!(labels(&variant_sheet(&live, None)), ["Delete"]);
    // Rename and Duplicate type a name starting from the variant's.
    let rename = &variant_sheet(&saved, None).rows[1];
    assert!(matches!(
        &rename.act,
        Act::Type(TypeSpec { field: NumberField::RenameVariant, initial }) if initial == "Arena"
    ));
}

#[test]
fn the_notices_and_the_delete_question_say_what_happened() {
    assert_eq!(renamed("Court"), "Renamed to Court.");
    assert_eq!(duplicated("Court"), "Copied as Court.");
    assert_eq!(deleted("Court"), "Deleted Court.");
    let s = sheet::question(Ask::DeleteVariant, "Court");
    assert_eq!(s.body, "Delete Court?");
    assert_eq!(s.rows[s.focus].label, "No");
}

/// A dialog's footer follows its focus: A Select on an action, Back alone
/// on a fact row (the open variant's "Open now"), and again on moving off it.
#[test]
fn the_footer_follows_focus_onto_a_fact_row() {
    use crate::forge_screens::screen::ForgeScreen;
    use crate::list_class::Rows;
    use crate::ui_kit::input::footer_legend;
    use crate::ui_kit::spec::Describe;

    let saved = row("a", "Arena", true, None);
    let sheet = variant_sheet(&saved, Some("ARENA"));
    let fact = sheet.rows.iter().position(|r| r.act == Act::Info).unwrap();
    let screen = ForgeScreen::new(Kind::Variant);
    screen.show(sheet).unwrap();
    let select = footer_legend(&screen.prompts_at(Some(0)));
    let back = footer_legend(&screen.prompts_at(Some(fact)));
    assert!(select.contains("Select"), "{select}");
    assert!(!back.contains("Select") && back.contains("Back"), "{back}");
    // The class's footer slot is rewritten on each focus change.
    let footer = screen.focused_footer(Some(fact as i16)).unwrap();
    assert_eq!(footer, screen.focused_footer(Some(0)).unwrap());
    // The spec's legend is the focused row's.
    let mut focused = screen.sheet();
    focused.focus = fact;
    screen.show(focused).unwrap();
    assert_eq!(screen.spec().prompts, screen.prompts_at(Some(fact)));
}
