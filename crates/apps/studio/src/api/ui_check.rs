//! `ui.check`: our screens checked in every state at 4:3 and 16:9, each
//! finding named ([`super::Provider`], registered in [`super::seams`]);
//! and `check.run`'s `ui` and `hud` checks. docs/editor/agent-api.md,
//! "Content checks".
//!
//! The checks are the kit's (`boot::ui_kit::findings::of_state`: fit,
//! overlap, safe area, contrast, glyphs, string ids, the checks on what
//! was drawn and the design system's), run as `ui.widgets` and
//! `ui.render` run them, with the unsaved edits
//! (`UiScreens::check_state`). The HUD's are `hud.check`'s
//! (`hud_edit::checks`) over the HUD tool's HUD, and the HUD kit's
//! (`hud_kit::check::all`) over our kit HUDs, as `h2 hud sheet` runs them.
//!
//! **Aspects.** The game draws menus and the HUD in a 640 by 480 frame
//! and the host fits it to the window (docs/graphics/high-resolution.md,
//! "HUD and UI"): a menu uniformly into a centred 4:3 area, so its layout
//! at 16:9 is its layout at 4:3; the HUD's Centered fit the same, and its
//! Pinned fit moves each corner group to its viewport edge at the same
//! scale. The checks run on the 640 by 480 frame and `aspects` says, per
//! aspect asked, which fit that is.

use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::checks::{Finding, Severity, counts};
use super::{MethodDef, P, Provider, Reply, def, err};
use crate::App;
use crate::app::UiScreens;
use crate::app::ui_screens::edit::ours::SCREENS_DIR;

/// The provider.
#[derive(Default)]
pub struct UiCheck;

const CHECK: MethodDef = def(
    m::UI_CHECK,
    "Check our screens (or one, or one state) in every state at 4:3 and 16:9 with the unsaved edits: every finding of the kit's checks (text overflowing its rect, overlapping texts, widgets outside the title-safe area, unreadable contrast, missing glyphs and string ids, art drawn outside its rect, the design system's rules) named by code (`ui.<check>.<kind>`), severity, the screen and state, the widgets and the screen's file. Draws nothing to disk; ui.render shows one",
    r#"{"screen?":"string","state?":"string","aspect?":"4:3|16:9|both (default both)"}"#,
    r#"{"findings":"[{area,code,severity,message,target,path?,screen,state,check,kind,ids}]","counts":"{error,warning,note,total,by_area}","checked":"[{screen,state,findings,skipped}]","aspects":"[{aspect,fit}]","from_disc":"boolean"}"#,
    r#"{"screen":"$SCREEN"}"#,
);

impl Provider for UiCheck {
    fn methods(&self) -> Vec<MethodDef> {
        vec![CHECK]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        (method == m::UI_CHECK).then(|| ui_check(app, &P(params)))
    }
}

/// What each aspect's layout is, for the menus (`hud` false) or the HUD.
pub fn aspects(which: &str, hud: bool) -> Result<Vec<Value>, String> {
    let four = json!({"aspect": "4:3", "fit": "the game's 640 by 480 frame"});
    let wide = json!({
        "aspect": "16:9",
        "fit": if hud {
            "Centered: the 4:3 layout in a centred 4:3 area; Pinned: each corner group at its viewport edge, the same scale (not modelled: groups only move apart)"
        } else {
            "the 4:3 layout, fitted uniformly into a centred 4:3 area"
        },
    });
    match which {
        "4:3" => Ok(vec![four]),
        "16:9" => Ok(vec![wide]),
        "both" => Ok(vec![four, wide]),
        other => Err(format!("aspect: 4:3, 16:9 or both (not {other:?})")),
    }
}

/// A screen's file in the checkout, when it has one (a baked doc).
fn screen_path(screen: &str) -> Option<String> {
    boot::ui_kit::data::baked(screen).map(|_| format!("{SCREENS_DIR}/{screen}.yaml"))
}

/// One of the kit's findings as a content finding: `consistency`'s are
/// warnings (the design system's rules), the rest errors (`h2 ui
/// preview` and the screens' tests fail on them).
pub fn finding_of(
    screen: &str,
    state: &str,
    f: &boot::ui_kit::findings::Found,
) -> (Finding, Value) {
    let severity = if f.check == "consistency" {
        Severity::Warning
    } else {
        Severity::Error
    };
    let base = Finding {
        area: "ui",
        code: format!("ui.{}.{}", f.check, f.kind),
        severity,
        message: f.text.clone(),
        target: format!("{screen}/{state}"),
        path: screen_path(screen),
    };
    let extra = json!({
        "screen": screen,
        "state": state,
        "check": f.check,
        "kind": f.kind,
        "ids": f.ids,
    });
    (base, extra)
}

/// A layout note (flagged, never failed) as a content finding.
fn note_of(screen: &str, state: &str, text: &str) -> (Finding, Value) {
    (
        Finding {
            area: "ui",
            code: "ui.layout.note".into(),
            severity: Severity::Note,
            message: text.to_owned(),
            target: format!("{screen}/{state}"),
            path: screen_path(screen),
        },
        json!({"screen": screen, "state": state, "check": "layout", "kind": "note", "ids": []}),
    )
}

/// What [`run_screens`] found.
#[derive(Default)]
pub struct Checked {
    pub findings: Vec<(Finding, Value)>,
    /// Each state checked: screen, state, its findings and the checks
    /// the source could not run.
    pub states: Vec<Value>,
}

/// Every screen of ours (or `screen`), every state (or `state`), checked.
pub fn run_screens(
    app: &mut App,
    screen: Option<&str>,
    state: Option<&str>,
) -> Result<Checked, String> {
    let tree = UiScreens::tree();
    if let Some(s) = screen
        && !tree.iter().any(|(n, _)| *n == s)
    {
        return Err(format!("no screen {s:?} (ui.screens lists them)"));
    }
    if state.is_some() && screen.is_none() {
        return Err("state names a state of one screen: give screen too".into());
    }
    app.sync_ui_disc();
    let mut out = Checked::default();
    for (name, states) in tree.iter().filter(|(n, _)| screen.is_none_or(|s| s == *n)) {
        if let Some(st) = state
            && !states.iter().any(|x| x == st)
        {
            return Err(format!(
                "{name} has no state {st:?} ({})",
                states.join(", ")
            ));
        }
        for st in states
            .iter()
            .filter(|x| state.is_none_or(|s| s == x.as_str()))
        {
            match app.ui_screens.check_state(name, st) {
                Ok(r) => {
                    let before = out.findings.len();
                    out.findings
                        .extend(r.findings.iter().map(|f| finding_of(name, st, f)));
                    out.findings
                        .extend(r.notes.iter().map(|n| note_of(name, st, n)));
                    out.states.push(json!({
                        "screen": name,
                        "state": st,
                        "findings": out.findings.len() - before,
                        "skipped": r.skipped,
                    }));
                }
                Err(e) => {
                    let text = format!("{name} in {st} did not draw: {e:#}");
                    out.findings.push((
                        Finding {
                            area: "ui",
                            code: "ui.draw".into(),
                            severity: Severity::Error,
                            message: text.clone(),
                            target: format!("{name}/{st}"),
                            path: screen_path(name),
                        },
                        json!({"screen": name, "state": st, "check": "draw", "kind": "failed", "ids": []}),
                    ));
                    out.states
                        .push(json!({"screen": name, "state": st, "findings": 1, "error": text}));
                }
            }
        }
    }
    Ok(out)
}

fn ui_check(app: &mut App, p: &P) -> Reply {
    let aspects = aspects(p.opt_str("aspect")?.unwrap_or("both"), false)
        .map_err(|e| err(code::INVALID_PARAMS, e))?;
    let checked = run_screens(app, p.opt_str("screen")?, p.opt_str("state")?)
        .map_err(|e| err(code::INVALID_PARAMS, e))?;
    let plain: Vec<Finding> = checked.findings.iter().map(|(f, _)| f.clone()).collect();
    let findings: Vec<Value> = checked
        .findings
        .iter()
        .map(|(f, extra)| {
            let mut v = f.json();
            if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
                o.extend(e.clone());
            }
            v
        })
        .collect();
    Ok(json!({
        "findings": findings,
        "counts": counts(&plain),
        "checked": checked.states,
        "aspects": aspects,
        "from_disc": app.ui_screens.checks_from_disc(),
    }))
}

/// `check.run`'s `ui`: [`run_screens`] over every screen, or the screen
/// `target` names (`screen` or `screen/state`).
pub fn check(app: &mut App, target: Option<&str>) -> Result<Vec<Finding>, String> {
    let (screen, state) =
        match target.map(|t| t.split_once('/').map_or((t, None), |(a, b)| (a, Some(b)))) {
            Some((s, st)) => (Some(s), st),
            None => (None, None),
        };
    Ok(run_screens(app, screen, state)?
        .findings
        .into_iter()
        .map(|(f, _)| f)
        .collect())
}

/// `check.run`'s `hud`: `hud.check`'s findings for the HUD the HUD tool
/// shows, at its state. `target`, when given, is that HUD's name.
pub fn hud_check(app: &mut App, target: Option<&str>) -> Result<Vec<Finding>, String> {
    let Some(session) = app.hud.session.as_ref() else {
        return Err("no map is open in the HUD tool (hud.list {\"map\": ...})".into());
    };
    let name = session.hud().map(|h| h.name.clone()).unwrap_or_default();
    if let Some(t) = target
        && t != name
    {
        return Err(format!("the HUD tool shows {name:?}, not {t:?}"));
    }
    let shot = app.hud.shot().ok_or("the HUD could not be drawn")?;
    Ok(shot
        .findings
        .iter()
        .map(|f| Finding {
            area: "hud",
            code: format!("hud.{}", f.code),
            severity: match f.severity {
                hud_edit::checks::Severity::Note => Severity::Note,
                hud_edit::checks::Severity::Warning => Severity::Warning,
            },
            message: f.text.clone(),
            target: name.clone(),
            path: None,
        })
        .collect())
}

/// `check.run`'s HUD kit: every kit HUD (or the one `target` names) in
/// every state and screen mode over the kit's synthetic map
/// (`hud_kit::Source::synthetic`), as `h2 hud sheet --synthetic` checks
/// them: each finding `hud.kit.<check>`, an error (the sheet fails on
/// it); each check the source could not run a note.
pub fn hud_kit_check(_app: &mut App, target: Option<&str>) -> Result<Vec<Finding>, String> {
    use hud_kit::Describe;
    let all = hud_kit::huds::all();
    let ds: Vec<&dyn Describe> = all
        .iter()
        .map(|d| d.as_ref())
        .filter(|d| target.is_none_or(|t| d.hud().name == t))
        .collect();
    if ds.is_empty() {
        return Err(format!(
            "no kit HUD {:?} (the kit's: {})",
            target.unwrap_or_default(),
            all.iter()
                .map(|d| d.hud().name)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let mut src =
        hud_kit::Source::synthetic().map_err(|e| format!("the kit's synthetic map: {e:#}"))?;
    let r =
        hud_kit::check::all(&mut src, &ds, &hud_kit::Mode::ALL).map_err(|e| format!("{e:#}"))?;
    let code_of = |name: &str| name.replace(' ', "_");
    let mut out: Vec<Finding> = r
        .findings
        .iter()
        .map(|f| Finding {
            area: "hud",
            code: format!("hud.kit.{}", code_of(f.check.name())),
            severity: Severity::Error,
            message: f.to_string(),
            target: f.hud.to_owned(),
            path: None,
        })
        .collect();
    out.extend(r.skipped.iter().map(|(c, why)| Finding {
        area: "hud",
        code: format!("hud.kit.{}.skipped", code_of(c.name())),
        severity: Severity::Note,
        message: format!(
            "the {} check did not run on the synthetic map: {why}",
            c.name()
        ),
        target: String::new(),
        path: None,
    }));
    Ok(out)
}

#[cfg(test)]
mod tests;
