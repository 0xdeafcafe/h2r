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
fn the_registry_covers_every_area() {
    for area in AREAS {
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

/// The models row is `model.check`'s: on the Pogo kit's invented map with
/// one look's first-person block taken out, `check.run` names it.
#[test]
fn check_run_reports_a_weapon_without_a_first_person_view() {
    let t = Temp::new("models");
    let mut app = App::new();
    app.set_data_root(t.0.join("data"));
    crate::api::models::tests::open_court(
        &mut app,
        &t.0.join("court.map"),
        &crate::api::models::tests::without_first_person(),
    );
    let mut api = Api::new();
    let r: Value = api
        .call(&mut app, m::CHECK_RUN, json!({"what": "models"}))
        .unwrap();
    assert_eq!(r["skipped"], json!([]), "{r}");
    assert_eq!(r["ran"][0]["check"], m::MODEL_CHECK);
    let findings = r["findings"].as_array().unwrap();
    assert!(findings.iter().all(|x| x["area"] == "models"), "{r}");
    let missing = findings
        .iter()
        .find(|x| x["code"] == "model.no_first_person")
        .unwrap_or_else(|| panic!("{r}"));
    assert_eq!(missing["severity"], "error");
    assert!(
        missing["target"].as_str().is_some_and(|t| t.ends_with("pogo_unsc")),
        "{missing}"
    );
    // The headroom once, against the map.
    let memory: Vec<_> = findings
        .iter()
        .filter(|x| x["code"] == "model.tag_memory")
        .collect();
    assert_eq!(memory.len(), 1, "{r}");
    // `target` narrows the row to one weapon: the look that keeps the
    // fixture pistol's view has no such finding.
    let r: Value = api
        .call(
            &mut app,
            m::CHECK_RUN,
            json!({"what": "models", "target": "pogo_covenant"}),
        )
        .unwrap();
    assert!(
        r["findings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|x| x["code"] != "model.no_first_person"),
        "{r}"
    );
}
