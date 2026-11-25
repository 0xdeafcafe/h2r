//! The Problems list over invented checks and invented packages (no game
//! data): findings from two sources list together, a row opens its
//! document and field, and packages report their own problems.

use std::path::Path;

use super::problems::{SOURCES, source_of};
use super::shell_tests::{Temp, app, kit_id};
use super::*;
use crate::api::checks::{ContentCheck, Finding, Severity};

const KIT: &str = "itmc:objects\\test\\kit";

fn finding(area: &'static str, severity: Severity, target: &str, message: &str) -> Finding {
    Finding {
        area,
        code: format!("{area}.invented"),
        severity,
        message: message.into(),
        target: target.into(),
        path: None,
    }
}

fn model_check(_: &mut App, _: Option<&str>) -> Result<Vec<Finding>, String> {
    Ok(vec![finding(
        "models",
        Severity::Warning,
        KIT,
        "count is nonsense (tag.set path count)",
    )])
}

fn ui_check(_: &mut App, _: Option<&str>) -> Result<Vec<Finding>, String> {
    Ok(vec![finding("ui", Severity::Error, "screen/one", "text overflows")])
}

fn hud_cannot(_: &mut App, _: Option<&str>) -> Result<Vec<Finding>, String> {
    Err("no map is open in the HUD tool".into())
}

const INVENTED: &[ContentCheck] = &[
    ContentCheck { area: "models", name: "model.check", run: model_check },
    ContentCheck { area: "ui", name: "ui.check", run: ui_check },
    ContentCheck { area: "hud", name: "hud.check", run: hud_cannot },
];

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Two packages: `bad` has a bitmap item whose file is not a picture, and
/// `broken` names a file it does not have.
fn packages(data: &Path) {
    let manifest = |id: &str, source: &str| {
        format!(
            "[package]\nformat = 1\nid = \"{id}\"\nname = \"{id}\"\n\n[[item]]\nkind = \"bitmap\"\nsource = \"{source}\"\ntag = \"ui\\\\{id}\"\n"
        )
    };
    let bad = data.join("packages/bad");
    write(&bad.join("package.toml"), &manifest("bad", "art/bad.png"));
    write(&bad.join("art/bad.png"), "not a picture");
    let broken = data.join("packages/broken");
    write(&broken.join("package.toml"), &manifest("broken", "art/gone.png"));
}

fn index_of(app: &App, source: &str) -> usize {
    app.problems.rows.iter().position(|p| p.source == source).unwrap()
}

#[test]
fn findings_from_two_sources_list_together_worst_first() {
    let t = Temp::new("problems-two");
    let mut app = app(&t.0);
    app.set_data_root(t.0.join("data"));
    for source in ["check", "ui", "hud"] {
        app.refresh_problems_over(Some(source), INVENTED);
    }
    let rows = &app.problems.rows;
    assert_eq!(rows.iter().map(|p| p.source).collect::<Vec<_>>(), ["ui", "check"]);
    assert_eq!(rows[0].severity, Severity::Error);
    assert_eq!(rows[1].asset.as_deref(), Some(&format!("tag:{KIT}")[..]));
    assert_eq!(rows[1].field.as_deref(), Some("count"));
    assert_eq!(rows[0].tool, Some(ToolKind::UiScreens));
    // A check that cannot run is said, not a row.
    assert_eq!(app.problems.skipped.len(), 1);
    assert_eq!(app.problems.skipped[0]["source"], "hud");
    assert_eq!(app.problems.counts(None)["total"], 2);
    assert_eq!(app.problems.counts(Some("ui"))["error"], 1);
    for s in SOURCES {
        assert_eq!(source_of(if s == "check" { "sounds" } else { s }), s);
    }
}

#[test]
fn opening_a_row_focuses_the_document_and_the_field() {
    let t = Temp::new("problems-open");
    let mut app = app(&t.0);
    app.set_data_root(t.0.join("data"));
    for source in ["check", "ui"] {
        app.refresh_problems_over(Some(source), INVENTED);
    }
    app.dispatch(Action::OpenProblem(index_of(&app, "check")));
    assert_eq!(app.front_doc(), Some(Doc::Tag(kit_id())));
    assert_eq!(app.nav_location().path.as_deref(), Some("count"));
    // A screen's finding opens the tool that holds the screen.
    app.dispatch(Action::OpenProblem(index_of(&app, "ui")));
    assert_eq!(app.tool, ToolKind::UiScreens);
    // A row that is not in the list is said in the log.
    let before = app.log().len();
    app.dispatch(Action::OpenProblem(99));
    assert!(app.log().len() > before);
}

#[test]
fn packages_report_item_findings_and_load_errors_and_open_in_the_browser() {
    let t = Temp::new("problems-packages");
    let mut app = app(&t.0);
    let data = t.0.join("data");
    app.set_data_root(data.clone());
    packages(&data);
    app.refresh_problems_over(Some("package"), &[]);
    let item = &app.problems.rows[index_of(&app, "package")];
    assert_eq!(item.asset.as_deref(), Some("item:bad:0"));
    assert_eq!(item.code, "package.item");
    assert_eq!(item.severity, Severity::Error);
    assert_eq!(item.path.as_deref(), Some("art/bad.png"));
    // The build source is what the set refuses to load; reading it keeps
    // the package rows.
    app.refresh_problems_over(Some("build"), &[]);
    let build: Vec<_> = app.problems.of(Some("build")).collect();
    assert_eq!(build.len(), 1);
    assert!(build[0].message.contains("broken"), "{}", build[0].message);
    assert_eq!(app.problems.of(Some("package")).count(), 1);
    // A package row opens in the browser.
    app.dispatch(Action::OpenProblem(index_of(&app, "package")));
    assert_eq!(app.problems.reveal.as_deref(), Some("item:bad:0"));
}

#[test]
fn the_open_script_lists_its_diagnostics_and_a_row_opens_its_line() {
    let t = Temp::new("problems-script");
    let mut app = app(&t.0);
    app.set_data_root(t.0.join("data"));
    // Nothing is listed while no script is open.
    app.refresh_problems_over(Some("check"), &[]);
    assert_eq!(app.problems.of(Some("check")).count(), 0);
    let package = app.data_root().join("packages/mode-test");
    write(
        &package.join("package.toml"),
        "[package]\nformat = 1\nid = \"mode-test\"\nname = \"Mode test\"\n\n[[item]]\nkind = \"rules\"\nsource = \"sample.rules\"\n",
    );
    let script = package.join("sample.rules");
    write(&script, "this is not a script {{{ ((");
    app.rules.path = Some(script);
    app.rules.set_text("this is not a script {{{ ((".into());
    app.rules.opened = true;
    let narrowed = app
        .refresh_asset_problems("item:mode-test:0", Some("check"), INVENTED)
        .unwrap();
    let expected = super::rules_edit::problems(&app.rules.text, &app.rules.analysis.diagnostics);
    assert!(!expected.is_empty());
    let rows: Vec<_> = app.problems.of(Some("check")).filter(|p| p.area == "scripts").collect();
    assert_eq!(rows.len(), expected.len());
    assert_eq!(narrowed.rows.iter().collect::<Vec<_>>(), rows);
    assert!(rows.iter().all(|p| p.at.is_some() && p.tool == Some(ToolKind::Variants)));
    // The header of the script reads the same rows.
    assert_eq!(app.doc_problem_count(Doc::Rules), expected.len());
    // Opening one shows the script editor with the cursor on the finding.
    let index = app.problems.rows.iter().position(|p| p.area == "scripts").unwrap();
    let (line, col) = app.problems.rows[index].at.unwrap();
    app.dispatch(Action::OpenProblem(index));
    assert_eq!(app.tool, ToolKind::Variants);
    let row = expected.iter().find(|r| r.line == line && r.col == col).unwrap();
    assert_eq!(app.rules.want_cursor, Some(row.at));
}

#[test]
fn a_documents_count_is_the_list_narrowed_to_its_asset() {
    let t = Temp::new("problems-count");
    let mut app = app(&t.0);
    app.set_data_root(t.0.join("data"));
    for source in ["check", "ui", "hud"] {
        app.refresh_problems_over(Some(source), INVENTED);
    }
    let asset = format!("tag:{KIT}");
    let rows = app.asset_problems(&asset);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].message, "count is nonsense (tag.set path count)");
    assert_eq!(app.doc_problem_count(Doc::Tag(kit_id())), rows.len());
    // The slash form names the same asset.
    assert_eq!(app.asset_problems(&asset.replace('\\', "/")).len(), 1);
}
