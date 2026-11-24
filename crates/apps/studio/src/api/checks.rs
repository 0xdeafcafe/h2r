//! `check.run`: every content check over the open document, map and our
//! own content, in one list ([`super::Provider`], registered in
//! [`super::seams`]). docs/editor/agent-api.md, "Content checks".
//!
//! A check is a row of [`CHECKS`]: its area (`models`, `sounds`, `ui`,
//! `hud`), its name (the method that shows the same findings in detail)
//! and a function from the app and the optional `target` to
//! [`Finding`]s, or why it could not run. Adding a check is one row.

use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::{MethodDef, P, Provider, Reply, def, err};
use crate::App;

/// How bad a finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Worth a look, often by design.
    Note,
    /// Likely wrong.
    Warning,
    /// Wrong: the check's own tool fails on it.
    Error,
}

impl Severity {
    pub fn name(self) -> &'static str {
        match self {
            Severity::Note => "note",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

/// One finding of a content check.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    /// The check's area: one of [`AREAS`].
    pub area: &'static str,
    /// What was found, dotted from the area (`sound.loud`,
    /// `ui.layout.unsafe`, `hud.kit.fit`).
    pub code: String,
    pub severity: Severity,
    pub message: String,
    /// What it is about: a tag, a clip id, `screen/state`, a HUD.
    pub target: String,
    /// The file in the checkout that holds it, when there is one.
    pub path: Option<String>,
}

impl Finding {
    pub fn json(&self) -> Value {
        let mut v = json!({
            "area": self.area,
            "code": self.code,
            "severity": self.severity.name(),
            "message": self.message,
            "target": self.target,
        });
        if let Some(p) = &self.path {
            v["path"] = json!(p);
        }
        v
    }
}

/// A check: its findings over the app (and `target`, when given), or why
/// it did not run.
pub type CheckFn = fn(&mut App, Option<&str>) -> Result<Vec<Finding>, String>;

/// One registered check.
pub struct ContentCheck {
    pub area: &'static str,
    /// The method whose findings these are, for the detail.
    pub name: &'static str,
    pub run: CheckFn,
}

/// The areas `what` may name, besides `all`.
pub const AREAS: [&str; 5] = ["models", "sounds", "ui", "hud", "markers"];

/// Every content check, in the order they run. One row each.
pub const CHECKS: &[ContentCheck] = &[
    ContentCheck {
        area: "models",
        name: m::MODEL_CHECK,
        run: super::models::check,
    },
    ContentCheck {
        area: "sounds",
        name: m::SOUND_MEASURE,
        run: super::sound_measure::check,
    },
    ContentCheck {
        area: "ui",
        name: m::UI_CHECK,
        run: super::ui_check::check,
    },
    ContentCheck {
        area: "hud",
        name: m::HUD_CHECK,
        run: super::ui_check::hud_check,
    },
    ContentCheck {
        area: "hud",
        name: "hud.kit",
        run: super::ui_check::hud_kit_check,
    },
    ContentCheck {
        area: "markers",
        name: "markers.check",
        run: super::markers_check::check,
    },
];

/// The provider.
#[derive(Default)]
pub struct Checks;

const RUN: MethodDef = def(
    m::CHECK_RUN,
    "Run every registered content check (or one area's: models, sounds, ui, hud, markers) over the open map and document and our own content, in one list: each finding's area, code, severity (error, warning, note), message, target and file. `target` narrows each check to one thing (an object tag, a clip id or prefix, a snd! tag, a screen, a HUD). A check that cannot run says why under `skipped`",
    r#"{"what?":"all|models|sounds|ui|hud|markers","target?":"string"}"#,
    r#"{"findings":"[{area,code,severity,message,target,path?}]","counts":"{error,warning,note,total,by_area:{area:integer}}","ran":"[{area,check,findings}]","skipped":"[{area,check?,why}]"}"#,
    r#"{"what":"ui","target":"$SCREEN"}"#,
);

impl Provider for Checks {
    fn methods(&self) -> Vec<MethodDef> {
        vec![RUN]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        (method == m::CHECK_RUN).then(|| run(app, &P(params), CHECKS))
    }
}

/// `check.run` over `checks`.
pub fn run(app: &mut App, p: &P, checks: &[ContentCheck]) -> Reply {
    let what = p.opt_str("what")?.unwrap_or("all");
    if what != "all" && !AREAS.contains(&what) {
        return Err(err(
            code::INVALID_PARAMS,
            format!("what: all, {} (not {what:?})", AREAS.join(", ")),
        ));
    }
    let target = p.opt_str("target")?;
    let mut findings: Vec<Finding> = Vec::new();
    let mut ran = Vec::new();
    let mut skipped = Vec::new();
    for area in AREAS.iter().filter(|a| what == "all" || **a == what) {
        let of_area: Vec<&ContentCheck> = checks.iter().filter(|c| c.area == *area).collect();
        if of_area.is_empty() {
            skipped
                .push(json!({"area": area, "why": format!("no check is registered for {area}")}));
            continue;
        }
        for c in of_area {
            match (c.run)(app, target) {
                Ok(found) => {
                    ran.push(json!({"area": area, "check": c.name, "findings": found.len()}));
                    findings.extend(found);
                }
                Err(why) => skipped.push(json!({"area": area, "check": c.name, "why": why})),
            }
        }
    }
    // Worst first, then by area and target, as found within those.
    findings.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then_with(|| {
                AREAS
                    .iter()
                    .position(|x| *x == a.area)
                    .cmp(&AREAS.iter().position(|x| *x == b.area))
            })
            .then_with(|| a.target.cmp(&b.target))
    });
    Ok(json!({
        "findings": findings.iter().map(Finding::json).collect::<Vec<_>>(),
        "counts": counts(&findings),
        "ran": ran,
        "skipped": skipped,
    }))
}

/// The findings counted by severity and area.
pub fn counts(findings: &[Finding]) -> Value {
    let n = |s: Severity| findings.iter().filter(|f| f.severity == s).count();
    let by_area: serde_json::Map<String, Value> = AREAS
        .iter()
        .map(|a| {
            (
                (*a).to_owned(),
                json!(findings.iter().filter(|f| f.area == *a).count()),
            )
        })
        .collect();
    json!({
        "error": n(Severity::Error),
        "warning": n(Severity::Warning),
        "note": n(Severity::Note),
        "total": findings.len(),
        "by_area": by_area,
    })
}

#[cfg(test)]
mod tests;
