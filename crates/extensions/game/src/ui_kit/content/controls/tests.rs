use super::*;
use crate::ui_kit::dialog::{DialogGeometry, DialogSpec, dialog_view};
use crate::ui_kit::spec::{DescriptionBlock, PaneText};
use crate::ui_kit::view::{Art, Geometry, Opener, PaneArt, Synthetic, view_of};

fn fixture() -> ScreenSpec {
    let mut s = ScreenSpec::new("example", "Options");
    s.inputs = vec![Input::A, Input::B, Input::Up, Input::Down];
    s.footer = true;
    s.prompts = vec![Prompt::new(&[Glyph::B], "Back")];
    s
}

fn draw(s: &ScreenSpec) -> View {
    if let Some(d) = &s.dialog {
        return dialog_view(
            s,
            d,
            &DialogGeometry::fallback_for(d.base),
            &Synthetic,
            "test",
        );
    }
    view_of(s, &Geometry::fallback(s.list), &Synthetic, "test")
}

fn has(findings: &[Finding], kind: &str, id: &str) -> bool {
    findings
        .iter()
        .any(|finding| finding.kind == kind && finding.id == id)
}

#[test]
fn shows_help_for_every_row_and_reads_deliberately_unavailable_values() {
    let mut s = fixture();
    s.description = Some(DescriptionBlock {
        rect: [-110, -220, -170, 220],
        font: 1,
        colour: [1.0; 3],
    });
    s.content.row_help = true;
    s.content.unavailable_values.push("row.1.value".into());
    s.rows = vec![
        RowSpec::action("Resume").describe("Return to the match."),
        RowSpec::info("Latency", "—").describe("Waiting for the first sample."),
    ];
    assert!(check(&s, &draw(&s)).is_empty());

    let mut without_widget = draw(&s);
    without_widget.widgets.retain(|w| w.id != "description");
    assert!(has(
        &check(&s, &without_widget),
        MISSING_CONTENT,
        "description"
    ));

    s.rows[1].description = None;
    s.content.unavailable_values.clear();
    let findings = check(&s, &draw(&s));
    assert!(has(&findings, MISSING_CONTENT, "row.1.description"));
    assert!(has(&findings, MISSING_CONTENT, "row.1.value"));
}

#[test]
fn catches_dead_actions_missing_labels_and_unsupported_prompt_inputs() {
    let mut s = fixture();
    s.rows = vec![
        RowSpec::action("Resume").press(Press::Nothing),
        RowSpec::action("").press(Press::Open(" ".into())),
    ];
    s.prompts.push(Prompt::new(&[Glyph::X], "Refresh"));
    s.prompts.push(Prompt::new(&[], "Open"));
    let findings = check(&s, &draw(&s));
    assert!(has(&findings, DEAD_CONTROL, "row.0"));
    assert!(has(&findings, DEAD_CONTROL, "row.1"));
    assert!(has(&findings, DEAD_CONTROL, "legend"));
    assert!(has(&findings, MISSING_CONTENT, "row.1.label"));
}

#[test]
fn a_list_with_other_rows_needs_focus_navigation() {
    let mut s = fixture();
    s.rows = vec![RowSpec::action("Resume"), RowSpec::action("Settings")];
    assert!(check(&s, &draw(&s)).is_empty());
    s.inputs
        .retain(|input| !matches!(input, Input::Up | Input::Down));
    assert!(has(&check(&s, &draw(&s)), DEAD_CONTROL, "list"));
}

#[test]
fn grouped_prompt_stays_live_when_one_direction_stops_at_an_endpoint() {
    let mut s = fixture();
    s.inputs.extend([Input::Left, Input::Right]);
    s.rows = vec![RowSpec::action("Open"), RowSpec::info("Status", "Ready")];
    s.prompts
        .push(Prompt::new(&[Glyph::DpadLeft, Glyph::DpadRight], "Move"));
    assert!(
        check_prompt_effects(
            &s,
            &[(Input::Left, false), (Input::Right, true), (Input::A, true)]
        )
        .is_empty()
    );

    let dead = check_prompt_effects(
        &s,
        &[
            (Input::Left, false),
            (Input::Right, false),
            (Input::A, false),
        ],
    );
    assert!(has(&dead, DEAD_CONTROL, "row.0"));
    assert!(has(&dead, DEAD_CONTROL, "legend"));
    assert!(check_reachability(&s, &[true, true]).is_empty());
    assert!(has(
        &check_reachability(&s, &[true, false]),
        DEAD_CONTROL,
        "row.1"
    ));
}

#[test]
fn empty_catalog_can_keep_its_filters_but_must_show_a_message() {
    let mut s = fixture();
    assert!(has(&check(&s, &draw(&s)), MISSING_CONTENT, "list"));

    s.rows.push(RowSpec::action("Filter"));
    s.content.empty_text = Some("library.empty".into());
    assert!(has(&check(&s, &draw(&s)), MISSING_CONTENT, "library.empty"));
    s.texts.push(PaneText {
        id: "library.empty".into(),
        rect: [-110, -220, -170, 220],
        font: 1,
        colour: [1.0; 3],
        right: false,
        text: "No items match this filter.".into(),
    });
    assert!(check(&s, &draw(&s)).is_empty());
}

#[test]
fn required_icon_must_draw_an_image_and_pickers_need_options() {
    let mut s = fixture();
    s.content.required_icons.push("art.0".into());
    s.content.picker = true;
    s.dialog = Some(DialogSpec::new("", Opener::MAIN_MENU));
    let findings = check(&s, &draw(&s));
    assert!(has(&findings, MISSING_CONTENT, "list"));
    assert!(has(&findings, MISSING_CONTENT, "art.0"));

    let row = RowSpec::action("Choose this");
    s.rows.push(row.clone());
    s.dialog.as_mut().unwrap().rows.push(row);
    s.art.push(PaneArt {
        art: Art::Skin {
            skin: 2,
            index: 0,
            image: 0,
        },
        at: [-190, 40],
        size: [16, 16],
        depth: 0,
    });
    assert!(check(&s, &draw(&s)).is_empty());

    let mut v = draw(&s);
    v.widgets
        .iter_mut()
        .find(|w| w.id == "art.0")
        .unwrap()
        .alpha = 0;
    assert!(has(&check(&s, &v), MISSING_CONTENT, "art.0"));
}
