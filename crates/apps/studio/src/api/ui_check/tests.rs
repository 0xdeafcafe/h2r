//! `ui.check` and the `ui` and `hud` content checks over invented
//! layouts: the kit's example screen with a text past the title-safe
//! area, a text too long for its rect and two texts on each other, and
//! `hud_edit`'s invented HUD with its stray bitmap (no game data).
use std::path::PathBuf;

use boot::ui_kit::findings::of_state;
use boot::ui_kit::preview;
use boot::ui_kit::view::{Justify, Kind, Layer, Widget};
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::{aspects, finding_of, hud_check};
use crate::App;
use crate::api::Api;
use crate::api::checks::Severity;
use crate::app::{Action, UiScreens};

/// The kit's example screen (`boot::ui_kit::screens`), clean in the
/// synthetic source.
const EXAMPLE: &str = "kit.example";

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2s-uicheck-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn text(id: &str, rect: [i16; 4], s: &str) -> Widget {
    Widget {
        id: id.into(),
        rect,
        layer: Layer::Content,
        alpha: 255,
        kind: Kind::Text {
            text: s.into(),
            font: 1,
            justify: Justify::Left,
            colour: [255, 255, 255],
        },
    }
}

#[test]
fn an_invented_layout_is_named_by_code() {
    let from = preview::From::Synthetic;
    let states = boot::ui_kit::screens::states(EXAMPLE).unwrap();
    let (state, spec) = &states[0];
    let clean = from.view(spec, state).unwrap();
    let r = of_state(&from, spec, &clean).unwrap();
    assert!(r.findings.is_empty(), "{:?}", r.findings);

    // Rects are top, left, bottom, right in UI units (y up).
    let mut faulty = clean.clone();
    // Above the title-safe area's top (204).
    faulty
        .widgets
        .push(text("stray", [236, -310, 220, -200], "Stray"));
    // Ten units square: the line cannot fit.
    faulty.widgets.push(text(
        "cramped",
        [100, -100, 90, -90],
        "A line far too long for its rect",
    ));
    // Two texts on the same spot.
    faulty
        .widgets
        .push(text("twin.a", [20, -60, 0, 60], "Twin"));
    faulty
        .widgets
        .push(text("twin.b", [20, -60, 0, 60], "Twin"));
    let r = of_state(&from, spec, &faulty).unwrap();
    let found: Vec<_> = r
        .findings
        .iter()
        .map(|f| finding_of(EXAMPLE, state, f))
        .collect();
    let has = |code: &str, id: &str| {
        found.iter().any(|(f, extra)| {
            f.code == code && extra["ids"].as_array().unwrap().iter().any(|x| x == id)
        })
    };
    assert!(has("ui.layout.unsafe", "stray"), "{found:?}");
    assert!(has("ui.layout.fit", "cramped"), "{found:?}");
    assert!(has("ui.layout.text_overlap", "twin.a"), "{found:?}");
    for (f, extra) in &found {
        assert_eq!(f.area, "ui");
        assert_eq!(f.target, format!("{EXAMPLE}/{state}"));
        assert_eq!(extra["screen"], EXAMPLE);
        let want = if extra["check"] == "consistency" {
            Severity::Warning
        } else {
            Severity::Error
        };
        assert_eq!(f.severity, want, "{f:?}");
        assert!(
            f.code
                .starts_with(&format!("ui.{}.", extra["check"].as_str().unwrap()))
        );
    }
}

#[test]
fn aspects_say_which_fit_each_is() {
    let both = aspects("both", false).unwrap();
    assert_eq!(both.len(), 2);
    assert_eq!(
        (both[0]["aspect"].clone(), both[1]["aspect"].clone()),
        (json!("4:3"), json!("16:9"))
    );
    assert!(both[1]["fit"].as_str().unwrap().contains("centred 4:3"));
    assert!(
        aspects("16:9", true).unwrap()[0]["fit"]
            .as_str()
            .unwrap()
            .contains("Pinned")
    );
    assert!(aspects("21:9", false).is_err());
}

fn test_app(t: &Temp) -> App {
    let mut app = App::new();
    app.set_data_root(t.0.join("data"));
    app
}

#[test]
fn ui_check_checks_each_state_of_a_screen() {
    let t = Temp::new("screen");
    let mut app = test_app(&t);
    let mut api = Api::new();
    let r: Value = api
        .call(&mut app, m::UI_CHECK, json!({"screen": EXAMPLE}))
        .unwrap();
    let states = UiScreens::tree()
        .into_iter()
        .find(|(n, _)| *n == EXAMPLE)
        .unwrap()
        .1;
    assert_eq!(r["checked"].as_array().unwrap().len(), states.len(), "{r}");
    assert_eq!(r["aspects"].as_array().unwrap().len(), 2);
    assert_eq!(r["from_disc"], false);
    assert_eq!(
        r["counts"]["total"],
        r["findings"].as_array().unwrap().len()
    );
    // Its first state is clean (the example screen's own test).
    assert_eq!(r["checked"][0]["findings"], 0, "{r}");
    let one: Value = api
        .call(
            &mut app,
            m::UI_CHECK,
            json!({"screen": EXAMPLE, "state": states[0], "aspect": "16:9"}),
        )
        .unwrap();
    assert_eq!(one["checked"].as_array().unwrap().len(), 1);
    assert_eq!(one["aspects"][0]["aspect"], "16:9");
    for bad in [
        json!({"screen": "no.such.screen"}),
        json!({"state": "x"}),
        json!({"screen": EXAMPLE, "state": "no state"}),
        json!({"screen": EXAMPLE, "aspect": "5:4"}),
    ] {
        assert_eq!(
            api.call(&mut app, m::UI_CHECK, bad.clone())
                .unwrap_err()
                .code,
            code::INVALID_PARAMS,
            "{bad}"
        );
    }
    // check.run's ui check takes screen/state as its target.
    let target = format!("{EXAMPLE}/{}", states[0]);
    let found = super::check(&mut app, Some(&target)).unwrap();
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn the_hud_check_names_the_stray_bitmap() {
    let t = Temp::new("hud");
    let mut app = test_app(&t);
    // No map in the HUD tool: the check cannot run, and says why.
    assert!(hud_check(&mut app, None).unwrap_err().contains("HUD tool"));
    let map = t.0.join("hudtest.map");
    std::fs::write(&map, hud_edit::fixture::build().0).unwrap();
    app.dispatch(Action::OpenSource(map));
    let mut api = Api::new();
    api.call(&mut app, m::HUD_LIST, json!({"map": "hudtest"}))
        .unwrap();
    let found = hud_check(&mut app, None).unwrap();
    assert!(!found.is_empty());
    assert!(
        found
            .iter()
            .all(|f| f.area == "hud" && f.code.starts_with("hud.")),
        "{found:?}"
    );
    assert!(
        found.iter().all(|f| f.target == "invented\\hud\\player"),
        "{found:?}"
    ); // hud.check names its findings' codes too.
    let listed: Value = api.call(&mut app, m::HUD_CHECK, Value::Null).unwrap();
    for (f, l) in found.iter().zip(listed.as_array().unwrap()) {
        assert_eq!(f.code, format!("hud.{}", l["code"].as_str().unwrap()));
    }
    // Another HUD named: skipped by name.
    assert!(hud_check(&mut app, Some("invented\\hud\\other")).is_err());
}
