use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ::settings::Value;

use super::*;
use crate::list_class::Rows;
use crate::ui::Chosen;
use crate::ui_kit::choice::example;
use crate::ui_kit::data::design;
use crate::ui_kit::ids;
use crate::ui_kit::page::{Focus, ListEffect, Page};
use crate::ui_kit::picker;
use crate::ui_kit::screens::{self, NAMES};
use crate::ui_kit::spec::{Describe, RowKind, ScreenSpec};

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
    assert_eq!(p.press_extra(Input::X).as_deref(), Some("cleared"));
    assert_eq!(p.press_extra(Input::Y), None);
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

/// A slot row (its doc text and help lone slots) is its binding's
/// label, value and help, read as the page shows them; A opens the
/// binding's pick titled by that label and saying that help, and left
/// and right do nothing. A slot the binding leaves empty is not drawn:
/// the rows after it move up, and the list's count is the rows drawn.
#[test]
fn a_slot_row_is_its_bindings_and_an_empty_slot_is_not_drawn() {
    use crate::ui_kit::screen::Filled;
    let fills: Arc<std::sync::Mutex<Vec<Option<Filled>>>> = Arc::new(std::sync::Mutex::new(vec![
        Some(Filled {
            label: "Bounce points".into(),
            shown: "3".into(),
            help: Some("Points for each superbounce".into()),
        }),
        Some(Filled {
            label: "Respawn Time".into(),
            shown: "5 seconds".into(),
            help: None,
        }),
    ]));
    let picked = Arc::new(AtomicUsize::new(usize::MAX));
    let slot = |n: usize| {
        let (f, p) = (Arc::clone(&fills), Arc::clone(&picked));
        move |b: Bindings| {
            b.slot(
                ["q.0", "q.1", "q.2"][n],
                move || f.lock().unwrap().get(n).cloned().flatten(),
                move || {
                    let p = Arc::clone(&p);
                    Ok(picker::Pick::new(
                        "ignored",
                        vec!["1".into(), "2".into(), "3".into()],
                        2,
                        "ignored",
                        Arc::new(move |i| {
                            p.store(i, Ordering::SeqCst);
                            format!("q.{n} -> {i}")
                        }),
                    ))
                },
            )
        }
    };
    let rows = (0..3)
        .map(|n| format!("  row.{n}:
    kind: pick
    text: "{{label}}"
    help: "{{help}}"
    bind: q.{n}
"))
        .collect::<String>();
    let binds = slot(2)(slot(1)(slot(0)(Bindings::new())));
    let p = built("", &rows, None, binds);
    // Two of three slots filled: two rows, the binding's.
    assert_eq!(Rows::count(&p), 2);
    let s = p.spec();
    let labels: Vec<&str> = s.rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(labels, ["Bounce points", "Respawn Time"]);
    assert_eq!(s.rows[0].cells(crate::ui_kit::spec::ValueStyle::Column).1.as_deref(), Some("3"));
    assert_eq!(s.rows[0].description.as_deref(), Some("Points for each superbounce"));
    assert_eq!(s.rows[1].description, None, "a setting has no help");
    assert_eq!(ScreenSpec { focus: 1, ..s.clone() }.description_text(), None);
    // A opens the binding's pick, titled and described by the fill.
    assert_eq!(Rows::press(&p, 0), Chosen::Open(picker::NAME));
    let pick = picker::current().expect("a pick");
    assert_eq!(pick.title, picker::title("Bounce points"));
    assert_eq!(pick.description, "Points for each superbounce");
    assert_eq!(pick.values, ["1", "2", "3"]);
    assert_eq!((pick.set)(0), "q.0 -> 0");
    assert_eq!(picked.load(Ordering::SeqCst), 0);
    assert_eq!(Rows::sideways(&p, 0, true), None, "no arrows on a slot");
    assert_eq!(Rows::press(&p, 2), Chosen::Nothing, "no third row");
    // The first slot emptied: the second moves up; its fill changing
    // redraws the help.
    let _ = Rows::live_text(&p);
    fills.lock().unwrap()[0] = None;
    assert!(Rows::live_text(&p).redraw);
    assert!(!Rows::live_text(&p).redraw, "once");
    assert_eq!(Rows::count(&p), 1);
    assert_eq!(p.spec().rows[0].label, "Respawn Time");
    assert_eq!(Rows::texts(&p, 1), Vec::new());
    // The legend: a slot is a pick row, so no CHANGE.
    let words: Vec<String> = p.spec().prompts.iter().map(|p| p.text.clone()).collect();
    assert_eq!(words, ["SELECT", "BACK"]);
}

/// A slot's doc and its binding must agree, and a header name that is a
/// lone slot is the name binding's, read as it is drawn.
#[test]
fn a_slot_and_a_bound_name_attach_or_name_their_field() {
    let slot_row = "  row.0:
    kind: pick
    text: "{label}"
    help: "{help}"
    bind: q.0
";
    let empty = || Bindings::new().slot("q.0", || None, || Err("none".into()));
    assert_eq!(error_at("", slot_row, None, Bindings::new().pick("q.0", String::new, || Err("x".into()))), "rows.row.0.bind");
    let half = slot_row.replace(""{help}"", ""What it does."");
    assert_eq!(error_at("", &half, None, empty()), "rows.row.0.help");
    let fixed = slot_row.replace(""{label}"", ""Fixed"").replace(""{help}"", ""What it does."");
    assert_eq!(error_at("", &fixed, None, empty()), "rows.row.0.bind", "a slot binding on a fixed row");
    // The name.
    let named = |binds: Bindings| {
        let yaml = format!("{}{slot_row}", head("")).replace("name: "Name"", "name: "{variant}"");
        let d = ScreenDoc::parse("kit.dialog", &yaml, design()).unwrap_or_else(|e| panic!("{e}
{yaml}"));
        from_doc("kit-dialog", "kit.dialog", ids::family("kit.dialog"), &d, None, binds)
    };
    match named(empty()) {
        Ok(_) => panic!("a lone slot name with no binding"),
        Err(e) => assert_eq!(e.at, "settings.name"),
    }
    let shown = Arc::new(std::sync::Mutex::new("Superbounce".to_string()));
    let s2 = Arc::clone(&shown);
    let p = named(empty().name(move || s2.lock().unwrap().clone())).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(Rows::count(&p), 0, "an empty slot");
    let texts = |p: &KitScreen| -> Vec<String> { p.spec().texts.iter().map(|t| t.text.clone()).collect() };
    assert_eq!(texts(&p), ["Superbounce", "What this window is for."]);
    *shown.lock().unwrap() = "Bounce Hunt".into();
    assert_eq!(texts(&p)[0], "Bounce Hunt");
    match from_doc("kit-dialog", "kit.dialog", ids::family("kit.dialog"), &doc("", slot_row), None, empty().name(String::new)) {
        Ok(_) => panic!("a name binding for a fixed name"),
        Err(e) => assert_eq!(e.at, "settings.name"),
    }
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

/// The settings dialogs that are screens of the game; every other
/// settings doc is a component's specimen (`design.*`, `layout.*`).
const PAGES: [&str; 6] = ["clankers", "firefight", "h1-options", "browser.filters", "kit.dialog", "variant-quick"];

/// Every screen's doc with a `settings:` block.
fn settings_docs() -> Vec<(&'static str, Arc<ScreenDoc>)> {
    NAMES
        .iter()
        .filter_map(|n| data::require(n).ok().filter(|d| d.settings.is_some()).map(|d| (*n, d)))
        .collect()
}

/// A doc's rows in order.
fn rows_of(d: &ScreenDoc) -> Vec<&RowDoc> {
    (0..d.rows.len()).map_while(|i| d.rows.get(&format!("row.{i}"))).collect()
}

/// Doc row `i` (`r`) of a screen whose spec is `s`, as drawn: its label
/// and help, the spec's for a slot (`{label}`, `{help}`); `None` for a
/// slot the spec does not draw.
fn as_drawn(r: &RowDoc, s: &ScreenSpec, i: usize) -> Option<(String, Option<String>)> {
    if !data::lone_slot(&r.text) {
        return Some((r.text.clone(), r.help.clone()));
    }
    s.rows.get(i).map(|row| (row.label.clone(), row.description.clone()))
}

/// A screen's first state.
fn first_spec(name: &str) -> ScreenSpec {
    screens::live(name)
        .and_then(|l| l.specs().into_iter().next())
        .map(|(_, s)| s)
        .unwrap_or_else(|| panic!("{name}: no state"))
}

/// A screen whose rows are mostly settings (two value rows, or half its
/// rows) is the shared dialog, or is in [`NOT_SHARED`] with why.
#[test]
fn every_settings_list_is_the_shared_dialog_or_says_why() {
    for (name, why) in NOT_SHARED {
        assert!(NAMES.contains(name), "{name} is no screen");
        assert!(!why.trim().is_empty(), "{name}");
        assert!(
            data::require(name).map_or(true, |d| d.settings.is_none()),
            "{name} is the shared dialog: drop it from NOT_SHARED"
        );
    }
    let docs = settings_docs();
    for (name, _) in &docs {
        assert!(
            PAGES.contains(name) || name.starts_with("design.") || name.starts_with("layout."),
            "{name}: a settings dialog not in PAGES"
        );
    }
    let mut lists = 0;
    for name in NAMES {
        if name.starts_with("design.") || name.starts_with("layout.") {
            continue;
        }
        let Some((_, s)) = screens::live(name).and_then(|l| l.specs().into_iter().next()) else {
            continue;
        };
        let values = s.rows.iter().filter(|r| matches!(r.kind, RowKind::Value { .. })).count();
        if values >= 2 || (values > 0 && values * 2 >= s.rows.len()) {
            lists += 1;
            let shared = docs.iter().any(|(n, _)| *n == name);
            assert!(
                shared || NOT_SHARED.iter().any(|(n, _)| *n == name),
                "{name}: {values} of {} rows are settings: give it a settings: doc \
                 (docs/game/ui-builder.md \"The settings dialog\") or list it in NOT_SHARED with why",
                s.rows.len()
            );
        }
    }
    assert!(lists >= PAGES.len(), "{lists} settings lists");
}

/// The top of every settings dialog is its doc's header, the same at
/// every focus, in the spec and as drawn.
#[test]
fn the_top_is_the_same_under_every_focus() {
    let docs = settings_docs();
    assert!(docs.len() >= PAGES.len(), "{} settings docs", docs.len());
    for (name, d) in &docs {
        let st = d.settings.as_ref().unwrap();
        let s = first_spec(name);
        let texts: Vec<(&str, &str)> = s.texts.iter().map(|t| (t.id.as_str(), t.text.as_str())).collect();
        let name_line = st.name.as_deref().unwrap_or_default();
        // A bound name (`{variant}`) is the binding's: the sample's.
        let name_line = match s.texts.first() {
            Some(t) if data::lone_slot(name_line) => t.text.as_str(),
            _ => name_line,
        };
        assert_eq!(texts, [("header.name", name_line), ("header.about", st.about())], "{name}");
        assert_eq!(s.art.len(), 1, "{name}: the header's icon");
        for i in 0..rows_of(d).len() {
            let f = ScreenSpec {
                focus: i,
                ..s.clone()
            };
            assert_eq!((&f.title, &f.art, &f.texts), (&s.title, &s.art, &s.texts), "{name} focus {i}");
        }
        let moved: Vec<_> = crate::ui_kit::consistency::settings_check(name)
            .into_iter()
            .filter(|f| f.ids.iter().any(|id| id == "title" || id.starts_with("art.") || id.starts_with("header.")))
            .collect();
        assert!(moved.is_empty(), "{name}: {moved:?}");
    }
}

/// The block under the rows is the focused row's help, at every focus,
/// two lines at most, between the last row and the legend.
#[test]
fn the_bottom_is_the_focused_rows_help() {
    for (name, d) in settings_docs() {
        let s = first_spec(name);
        for (i, r) in rows_of(&d).iter().enumerate() {
            let Some((_, help)) = as_drawn(r, &s, i) else { continue };
            let f = ScreenSpec {
                focus: i,
                ..s.clone()
            };
            assert_eq!(f.description_text(), help, "{name} focus {i}");
        }
        let bottom: Vec<_> = crate::ui_kit::consistency::settings_check(name)
            .into_iter()
            .filter(|f| f.ids.iter().any(|id| id == "description"))
            .collect();
        assert!(bottom.is_empty(), "{name}: {bottom:?}");
    }
    for name in PAGES {
        let page = screens::list_page(name).unwrap_or_else(|| panic!("{name}: no page"));
        let d = data::require(name).unwrap();
        for (i, r) in rows_of(&d).iter().enumerate() {
            let Some((_, help)) = as_drawn(r, &page.spec(&Focus(0)), i) else { continue };
            assert_eq!(page.spec(&Focus(i)).description_text(), help, "{name} row {i}");
        }
    }
}

/// A on every value and pick row opens the value picker, titled by the
/// row, saying the row's explain, else its help.
#[test]
fn a_rows_picker_carries_its_help() {
    for name in PAGES {
        let page = screens::list_page(name).unwrap_or_else(|| panic!("{name}: no page"));
        let d = data::require(name).unwrap();
        let mut pickers = 0;
        for (i, r) in rows_of(&d).iter().enumerate() {
            if !matches!(r.kind.as_str(), "value" | "pick") {
                continue;
            }
            let Some((label, help)) = as_drawn(r, &page.spec(&Focus(0)), i) else { continue };
            pickers += 1;
            let mut focus = Focus(i);
            assert_eq!(page.step(&mut focus, Input::A), ListEffect::Open(picker::NAME), "{name} row {i}");
            let pick = picker::current().expect("a pick");
            assert_eq!(pick.title, picker::title(&label), "{name} row {i}");
            let says = r.explain.clone().or(help).unwrap_or_default();
            assert_eq!(pick.description, says, "{name} row {i}");
            assert!(!pick.values.is_empty(), "{name} row {i}");
        }
        assert!(pickers > 0, "{name}: no picker rows");
    }
}

/// No settings dialog draws a value between arrows: a value is chosen in
/// the picker A opens.
#[test]
fn no_settings_value_is_drawn_between_arrows() {
    for (name, _) in settings_docs() {
        let s = first_spec(name);
        for r in &s.rows {
            if let RowKind::Value { values, .. } = &r.kind {
                for v in values {
                    let t = v.trim();
                    assert!(!(t.starts_with('<') || t.ends_with('>')), "{name} {}: {v:?}", r.label);
                }
            }
        }
    }
}
