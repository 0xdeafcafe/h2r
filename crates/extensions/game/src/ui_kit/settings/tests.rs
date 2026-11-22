use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ::settings::Value;

use super::*;
use crate::list_class::Rows;
use crate::ui::Chosen;
use crate::ui_kit::choice::example;
use crate::ui_kit::data::design;
use crate::ui_kit::ids;
use crate::ui_kit::picker;
use crate::ui_kit::spec::Describe;

/// A settings doc's head, on the kit's example settings, before its rows.
fn head(extra: &str) -> String {
    format!(
        "screen: kit.dialog\nkind: dialog\nlayout: settings\nbacking: over\nopener: main-menu\nsettings:\n  host: menu\n  icon: question\n  name: \"Name\"\n  about: \"What this window is for.\"\n{extra}widgets:\n  title:\n    component: title\n    text: \"CHOICES\"\nrows:\n"
    )
}

fn doc(extra: &str, rows: &str) -> ScreenDoc {
    let yaml = format!("{}{rows}", head(extra));
    ScreenDoc::parse("kit.dialog", &yaml, design()).unwrap_or_else(|e| panic!("{e}\n{yaml}"))
}

fn page(
    extra: &str,
    rows: &str,
    store: Option<Arc<Store>>,
    binds: Bindings,
) -> Result<KitScreen, DataError> {
    from_doc("kit-dialog", "kit.dialog", ids::family("kit.dialog"), &doc(extra, rows), store, binds)
}

fn built(extra: &str, rows: &str, store: Option<Arc<Store>>, binds: Bindings) -> KitScreen {
    page(extra, rows, store, binds).unwrap_or_else(|e| panic!("{e}"))
}

fn error_at(extra: &str, rows: &str, store: Option<Arc<Store>>, binds: Bindings) -> String {
    match page(extra, rows, store, binds) {
        Ok(_) => panic!("no error for:\n{rows}"),
        Err(e) => e.at,
    }
}

const COUNT: &str = "  row.0:\n    kind: value\n    text: \"Count\"\n    help: \"How many join the game.\"\n    setting: kit.example_count\n";

/// A `setting:` row needs no Rust: its values are its def's choices (or
/// the doc's names for them), it reads the store and writes it.
#[test]
fn a_setting_row_reads_and_writes_its_def() {
    let store = example::store();
    let rows = format!(
        "{COUNT}  row.1:\n    kind: value\n    text: \"Difficulty\"\n    values: [\"E\", \"N\", \"H\", \"L\"]\n    help: \"How well they play.\"\n    setting: kit.example_level\n    explain: \"How well they play, from Easy to Legendary.\"\n"
    );
    let p = built("", &rows, Some(Arc::clone(&store)), Bindings::new());
    assert_eq!(
        Rows::sideways(&p, 0, true).as_deref(),
        Some("kit.dialog: right on row 0: kit.example_count -> 5")
    );
    assert_eq!(store.get("kit.example_count"), Some(Value::Int(5)));
    // A opens the value picker on the def's values, the doc's names
    // shown, the held one focused, with the row's explain.
    assert_eq!(Rows::press(&p, 1), Chosen::Open(picker::NAME));
    let pick = picker::current().expect("a pick");
    assert_eq!(pick.values, ["E", "N", "H", "L"]);
    assert_eq!(pick.current, 1);
    assert_eq!(pick.description, "How well they play, from Easy to Legendary.");
    // The row reads the store as it is now.
    store.set("kit.example_level", Value::Int(3)).unwrap();
    assert_eq!(Rows::press(&p, 1), Chosen::Open(picker::NAME));
    assert_eq!(picker::current().expect("a pick").current, 3);
    // With no explain, the picker says the row's help.
    assert_eq!(Rows::press(&p, 0), Chosen::Open(picker::NAME));
    let pick = picker::current().expect("a pick");
    assert_eq!(pick.description, "How many join the game.");
    assert_eq!(pick.values.len(), 9, "0 to 8");
}

/// The legend is the rows': CHANGE only with a value row, then SELECT
/// and BACK, then the doc's extra buttons.
#[test]
fn the_legend_follows_the_rows() {
    let words = |p: &KitScreen| -> Vec<String> { p.spec().prompts.iter().map(|p| p.text.clone()).collect() };
    let p = built("", COUNT, Some(example::store()), Bindings::new());
    assert_eq!(words(&p), ["CHANGE", "SELECT", "BACK"]);
    let pick = "  row.0:\n    kind: pick\n    text: \"Two\"\n    help: \"What two does.\"\n    bind: t.two\n";
    let binds = || {
        Bindings::new()
            .pick("t.two", || "A".into(), || Err("nothing".into()))
            .action("t.clear", || Then::Stay("cleared".into()))
    };
    let actions = "  actions:\n    - { button: X, words: \"CLEAR\", bind: t.clear }\n";
    let p = built(actions, pick, None, binds());
    assert_eq!(words(&p), ["SELECT", "BACK", "CLEAR"]);
    assert_eq!(legend(&doc(actions, pick)).len(), 2, "the extras are the screen's, not the doc legend's");
}

/// Each kind of bound row runs what its feature registered, an open row
/// opens its screen, and an extra button runs its binding.
#[test]
fn bound_rows_and_buttons_run_their_bindings() {
    let held = Arc::new(AtomicUsize::new(0));
    let (get, set) = (Arc::clone(&held), Arc::clone(&held));
    let binds = Bindings::new()
        .value(
            "t.one",
            vec!["Off".into(), "On".into()],
            move || get.load(Ordering::SeqCst),
            move |i| {
                set.store(i, Ordering::SeqCst);
                format!("t.one -> {i}")
            },
        )
        .pick(
            "t.two",
            || "Any".into(),
            || {
                Ok(picker::Pick::new(
                    "ignored",
                    vec!["Any".into(), "Some".into()],
                    0,
                    "the feature's line",
                    Arc::new(|i| format!("t.two -> {i}")),
                ))
            },
        )
        .action("t.go", || Then::Stay("went".into()))
        .action("t.clear", || Then::Stay("cleared".into()));
    let rows = "  row.0:\n    kind: value\n    text: \"One\"\n    help: \"What one does.\"\n    bind: t.one\n  row.1:\n    kind: pick\n    text: \"Two\"\n    help: \"What two does.\"\n    bind: t.two\n    explain: \"More about two.\"\n  row.2:\n    kind: action\n    text: \"Go\"\n    help: \"What going does.\"\n    bind: t.go\n  row.3:\n    kind: open\n    text: \"More...\"\n    help: \"Opens more.\"\n    open: kit.dialog.large\n";
    let actions = "  actions:\n    - { button: X, words: \"CLEAR\", bind: t.clear }\n";
    let p = built(actions, rows, None, binds);
    assert_eq!(Rows::sideways(&p, 0, true).as_deref(), Some("kit.dialog: right on row 0: t.one -> 1"));
    assert_eq!(held.load(Ordering::SeqCst), 1);
    // A pick row's picker is titled by the row and says its explain.
    assert_eq!(Rows::press(&p, 1), Chosen::Open(picker::NAME));
    let pick = picker::current().expect("a pick");
    assert_eq!(pick.title, "TWO");
    assert_eq!(pick.description, "More about two.");
    assert_eq!(Rows::press(&p, 2), Chosen::Stepped("went".into()));
    assert_eq!(Rows::press(&p, 3), Chosen::Open("kit.dialog.large"));
    assert_eq!(p.press_extra(crate::ui_kit::input::Input::X).as_deref(), Some("cleared"));
    assert_eq!(p.press_extra(crate::ui_kit::input::Input::Y), None);
}

/// Whatever doesn't attach is a named error at its field: an unknown
/// setting, a values override of the wrong length, a binding no one
/// registered or of the wrong kind, a binding no row names, a gap in the
/// rows, an open row to no screen.
#[test]
fn what_does_not_attach_is_a_named_error() {
    let store = || Some(example::store());
    let value = |extra: &str| {
        format!("  row.0:\n    kind: value\n    text: \"X\"\n    help: \"What X does.\"\n{extra}")
    };
    assert_eq!(error_at("", &value("    setting: no.such\n"), store(), Bindings::new()), "rows.row.0.setting");
    assert_eq!(error_at("", COUNT, None, Bindings::new()), "rows.row.0.setting", "no store");
    assert_eq!(
        error_at("", &value("    values: [\"A\"]\n    setting: kit.example_level\n"), store(), Bindings::new()),
        "rows.row.0.values"
    );
    assert_eq!(error_at("", &value("    bind: t.x\n"), store(), Bindings::new()), "rows.row.0.bind");
    assert_eq!(
        error_at("", &value("    bind: t.x\n"), store(), Bindings::new().action("t.x", || Then::Close)),
        "rows.row.0.bind",
        "an action binding on a value row"
    );
    assert_eq!(
        error_at("", COUNT, store(), Bindings::new().action("t.left", || Then::Close)),
        "rows",
        "a binding no row names"
    );
    let gap = COUNT.to_string() + &COUNT.replace("row.0", "row.2");
    assert_eq!(error_at("", &gap, store(), Bindings::new()), "rows");
    let open = "  row.0:\n    kind: open\n    text: \"More...\"\n    help: \"Opens more.\"\n    open: no.such\n";
    assert_eq!(error_at("", open, store(), Bindings::new()), "rows.row.0.open");
    let actions = "  actions:\n    - { button: X, words: \"CLEAR\", bind: t.clear }\n";
    assert_eq!(error_at(actions, COUNT, store(), Bindings::new()), "settings.actions.0.bind");
}

/// Every settings doc's picker copy fits the picker's description
/// block: a row's explain, else its help, is at most [`EXPLAIN_UNITS`].
#[test]
fn every_rows_picker_copy_fits() {
    let mut seen = 0;
    for name in data::names() {
        let d = data::require(name).unwrap_or_else(|e| panic!("{e}"));
        if d.settings.is_none() {
            continue;
        }
        seen += 1;
        for (id, r) in &d.rows {
            let text = r.explain.as_deref().or(r.help.as_deref()).unwrap_or_default();
            assert!(
                text.encode_utf16().count() <= EXPLAIN_UNITS,
                "{name} {id}: {} units: {text}",
                text.encode_utf16().count()
            );
        }
    }
    assert!(seen >= 5, "clankers, firefight, h1-options, browser.filters, kit.dialog at least");
}
