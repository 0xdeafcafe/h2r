use super::*;
use crate::ui_kit::{preview, spec::RowSpec, view::Synthetic};

#[test]
fn broken_strings_name_the_problem() {
    for (text, kind) in [
        ("", "empty_text"),
        ("   ", "empty_text"),
        ("ui.map_selection", "raw_key"),
        ("MENU_EMPTY_ITEMS", "raw_key"),
        ("?", "raw_key"),
        (">", "value_arrows"),
        ("Invite {player}", "placeholder"),
        ("Welcome %s", "placeholder"),
        ("Found %2$d items", "placeholder"),
        ("Hello $name", "placeholder"),
        ("Map 0x12345678", "raw_id"),
        ("TODO description", "unfinished_text"),
        ("FIXME", "unfinished_text"),
        ("xxx", "unfinished_text"),
        ("R\u{fffd}gle", "invalid_text"),
        ("CafÃ©", "invalid_text"),
        ("Use  this", "text_spacing"),
        ("Use this ", "text_spacing"),
        ("< All >", "value_arrows"),
    ] {
        let findings = check_text("row.0.label", text, true);
        assert!(
            findings
                .iter()
                .any(|f| f.kind == kind && f.id == "row.0.label"),
            "{text:?}: {findings:?}"
        );
    }
}

#[test]
fn authored_text_accepts_percentages_accents_and_footer_spacing() {
    for text in [
        "100% ready",
        "$10",
        "Café",
        "Xbox Live",
        "Change Map > Mods",
        "Use this\r\nThen that",
        "\u{e100} SELECT  \u{e101} BACK",
        "CTF",
    ] {
        assert!(check_text("text", text, true).is_empty(), "{text:?}");
    }
    assert!(check_text("optional", "", false).is_empty());
    assert!(
        check_text("legend", "\u{e100} SELECT  THIS", true)
            .iter()
            .any(|f| f.kind == "text_spacing")
    );
}

#[test]
fn capitalization_compares_row_labels_and_accepts_short_acronyms() {
    let findings = |labels: &[&str]| {
        case_findings(
            labels
                .iter()
                .enumerate()
                .map(|(i, label)| (format!("row.{i}.label"), (*label).into())),
        )
    };
    assert_eq!(
        findings(&["Sort by Date", "GAME TYPE"])[0].kind,
        "inconsistent_case"
    );
    assert_eq!(
        findings(&["Sort by Date", "all maps"])[0].kind,
        "inconsistent_case"
    );
    assert!(findings(&["Sort by Date", "Game Type", "CTF"]).is_empty());
    assert!(findings(&["SORT BY DATE", "GAME TYPE"]).is_empty());
}

struct FontFixture;

impl Metrics for FontFixture {
    fn width(&self, font: i16, text: &str) -> i32 {
        Synthetic.width(font, text)
    }
    fn line_height(&self, font: i16) -> i32 {
        Synthetic.line_height(font)
    }
    fn has_glyph(&self, _: i16, ch: char) -> bool {
        ch != '★'
    }
}

#[test]
fn font_coverage_and_fit_use_the_widget_font_and_bounds() {
    let mut spec = ScreenSpec::new("sample", "Library");
    spec.rows.push(RowSpec::action("Browse"));
    let make = |spec: &ScreenSpec| preview::From::Synthetic.view(spec, "rest").unwrap();
    assert!(view_text(&make(&spec), &FontFixture).is_empty());
    spec.rows[0].label = format!("★ {}", "W".repeat(80));
    let findings = view_text(&make(&spec), &FontFixture);
    for kind in ["missing_glyph", "truncated_text"] {
        assert!(
            findings
                .iter()
                .any(|f| f.kind == kind && f.id == "row.0.label"),
            "{findings:?}"
        );
    }
}

#[test]
fn a_declared_icon_must_resolve_when_the_source_has_assets() {
    let mut spec = ScreenSpec::new("sample", "Library");
    spec.art = crate::ui_kit::screens::skulls_art();
    spec.content.required_icons.push("art.1".into());
    let view = preview::From::Synthetic.view(&spec, "rest").unwrap();
    assert!(check_icons(&spec, &view, |_| None).is_empty());
    assert!(check_icons(&spec, &view, |_| Some(true)).is_empty());
    let found = check_icons(&spec, &view, |_| Some(false));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, "art.1");
}

/// Start each route from the same named state. Only the offline model is
/// driven: pages can decline probes that would use a worker or perform I/O.
fn audit_state(name: &str, state: &str) -> Vec<Finding> {
    use crate::ui_kit::{input::Input, page::Live, screens};

    let open = || -> Box<dyn Live> {
        let mut live = screens::live(name).expect("registered page");
        assert!(live.go(state), "{name}/{state}");
        live
    };
    let start = open().spec();
    let modal_rows = start.dialog.as_ref().map(|d| &d.rows);
    let row_count = modal_rows.unwrap_or(&start.rows).len();
    let mut reached = vec![None; row_count];
    for direction in [Input::Down, Input::Up] {
        let mut live = open();
        let mut visited = vec![false; row_count];
        for distance in 0..row_count {
            let spec = live.spec();
            let Some(seen) = visited.get_mut(spec.focus) else {
                break;
            };
            if spec.name != start.name || *seen {
                break;
            }
            *seen = true;
            reached[spec.focus].get_or_insert((direction, distance));
            let _ = live.audit_press(direction);
        }
    }
    let flags: Vec<_> = reached.iter().map(Option::is_some).collect();
    let mut out = controls::check_reachability(&start, &flags);
    let mut audit = |nav: Option<(Input, usize)>| {
        let go = || {
            let mut live = open();
            if let Some((direction, distance)) = nav {
                for _ in 0..distance {
                    let _ = live.audit_press(direction);
                }
            }
            live
        };
        let spec = go().spec();
        let source = preview::From::Synthetic;
        let view = source.view(&spec, state).unwrap();
        let results: Vec<_> = [
            Input::A,
            Input::B,
            Input::Up,
            Input::Down,
            Input::Left,
            Input::Right,
            Input::Start,
            Input::Back,
            Input::X,
            Input::Y,
        ]
        .into_iter()
        .filter(|&input| controls::promises_input(&spec, input))
        .filter_map(|input| go().audit_press(input).map(|handled| (input, handled)))
        .collect();
        for finding in check(&spec, &view, source.source())
            .into_iter()
            .chain(controls::check_prompt_effects(&spec, &results))
        {
            if !out.contains(&finding) {
                out.push(finding);
            }
        }
    };
    audit(None);
    for nav in reached
        .into_iter()
        .flatten()
        .filter(|(_, distance)| *distance != 0)
    {
        audit(Some(nav));
    }
    out
}

#[test]
fn every_registered_page_and_state_has_complete_content() {
    use crate::ui_kit::screens::{self, NAMES};

    let mut issues = Vec::new();
    for name in NAMES {
        let states = screens::live(name).expect("registered page").state_names();
        assert!(!states.is_empty(), "{name}");
        for state in states {
            for issue in audit_state(name, &state) {
                issues.push(format!("{name}/{state} {}: {issue}", issue.kind));
            }
        }
    }
    assert!(issues.is_empty(), "{}", issues.join("\n"));
}
