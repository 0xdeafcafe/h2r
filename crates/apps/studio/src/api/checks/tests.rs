//! `check.run` over invented checks (the registry's shape, the counts,
//! the order and what is skipped) and over the real ones on the sound
//! crate's invented map (no game data).
use std::path::PathBuf;

use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::{AREAS, CHECKS, ContentCheck, Finding, Severity, run};
use crate::App;
use crate::api::{Api, P};
use crate::app::tag_tests;

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2s-checks-{tag}-{}", std::process::id()));
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

fn open(t: &Temp) -> App {
    let (mut app, ..) = tag_tests::app_with_registry(
        &t.0,
        crate::api::sound::tests::sound_map(),
        tag_tests::registry(),
    );
    app.audition.headless = true;
    app
}

fn f(area: &'static str, code: &str, severity: Severity, target: &str) -> Finding {
    Finding {
        area,
        code: code.into(),
        severity,
        message: format!("{code} on {target}"),
        target: target.into(),
        path: None,
    }
}

fn two_sounds(_: &mut App, target: Option<&str>) -> Result<Vec<Finding>, String> {
    Ok(vec![
        f(
            "sounds",
            "sound.clipping",
            Severity::Warning,
            target.unwrap_or("b"),
        ),
        f("sounds", "sound.loud", Severity::Error, "a"),
    ])
}

fn ui_note(_: &mut App, _: Option<&str>) -> Result<Vec<Finding>, String> {
    Ok(vec![f("ui", "ui.layout.note", Severity::Note, "s/one")])
}

fn cannot(_: &mut App, _: Option<&str>) -> Result<Vec<Finding>, String> {
    Err("no map is open in the HUD tool".into())
}

/// A check registers with one row; one that cannot run is skipped with
/// its reason; an area with no row says so.
const INVENTED: &[ContentCheck] = &[
    ContentCheck {
        area: "sounds",
        name: "sound.measure",
        run: two_sounds,
    },
    ContentCheck {
        area: "ui",
        name: "ui.check",
        run: ui_note,
    },
    ContentCheck {
        area: "hud",
        name: "hud.check",
        run: cannot,
    },
];

#[test]
fn every_check_runs_into_one_list_worst_first() {
    let t = Temp::new("all");
    let mut app = open(&t);
    let params = json!({});
    let r = run(&mut app, &P(&params), INVENTED).unwrap();
    let codes: Vec<&str> = r["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, ["sound.loud", "sound.clipping", "ui.layout.note"]);
    assert_eq!(r["findings"][0]["severity"], "error");
    assert_eq!(r["findings"][0]["area"], "sounds");
    assert!(r["findings"][0].get("path").is_none());
    assert_eq!(
        r["counts"],
        json!({"error": 1, "warning": 1, "note": 1, "total": 3,
               "by_area": {"models": 0, "sounds": 2, "ui": 1, "hud": 0}})
    );
    assert_eq!(
        r["ran"],
        json!([{"area": "sounds", "check": "sound.measure", "findings": 2},
               {"area": "ui", "check": "ui.check", "findings": 1}])
    );
    assert_eq!(
        r["skipped"],
        json!([{"area": "models", "why": "no check is registered for models"},
               {"area": "hud", "check": "hud.check", "why": "no map is open in the HUD tool"}])
    );
}

#[test]
fn what_and_target_narrow_the_run() {
    let t = Temp::new("what");
    let mut app = open(&t);
    let params = json!({"what": "sounds", "target": "pogo/spring"});
    let r = run(&mut app, &P(&params), INVENTED).unwrap();
    assert_eq!(r["counts"]["total"], 2);
    assert_eq!(r["skipped"], json!([]));
    assert!(
        r["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["target"] == "pogo/spring")
    );
    let params = json!({"what": "models"});
    let r = run(&mut app, &P(&params), INVENTED).unwrap();
    assert_eq!(r["counts"]["total"], 0);
    assert_eq!(r["skipped"][0]["area"], "models");
    let params = json!({"what": "everything"});
    assert_eq!(
        run(&mut app, &P(&params), INVENTED).unwrap_err().code,
        code::INVALID_PARAMS
    );
}

#[test]
fn the_registry_covers_the_sounds_ui_and_hud() {
    // The models row is the model check's (one line in CHECKS).
    for area in ["sounds", "ui", "hud"] {
        assert!(CHECKS.iter().any(|c| c.area == area), "{area}");
    }
    assert!(CHECKS.iter().all(|c| AREAS.contains(&c.area)));
}

#[test]
fn check_run_answers_through_the_api() {
    let t = Temp::new("api");
    let mut app = open(&t);
    let mut api = Api::new();
    let r: Value = api
        .call(
            &mut app,
            m::CHECK_RUN,
            json!({"what": "sounds", "target": r"snd!:objects\test\blaster"}),
        )
        .unwrap();
    assert_eq!(r["ran"][0]["check"], m::SOUND_MEASURE);
    let findings = r["findings"].as_array().unwrap();
    assert!(findings.iter().all(|x| x["area"] == "sounds"), "{r}");
    // The map's only sound has nothing to be compared with.
    assert!(
        findings.iter().any(|x| x["code"] == "sound.no_reference"),
        "{r}"
    );
    // The HUD tool has no map open: its check is skipped, the kit's runs.
    let r: Value = api
        .call(&mut app, m::CHECK_RUN, json!({"what": "hud"}))
        .unwrap();
    assert!(
        r["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["check"] == m::HUD_CHECK),
        "{r}"
    );
    assert!(
        r["ran"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["check"] == "hud.kit"),
        "{r}"
    );
}
