//! `ui.*`: the UI Screens tool. Our screens drawn offline (from the disc
//! when one is open, else the synthetic source), stepped by pad presses,
//! their widgets' rects, and a PNG of the screen (CPU drawn: no GPU).

use std::path::PathBuf;

use boot::ui_kit::input::Input;
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::scenario::refused;
use super::{P, Reply, err};
use crate::App;
use crate::app::{Action, Pick, ToolKind, UiScreens};

fn pick_json(p: Option<&Pick>) -> Value {
    match p {
        Some(Pick::Native { screen, state }) => json!({ "screen": screen, "state": state }),
        Some(Pick::Retail(tag)) => json!({ "retail": tag }),
        None => Value::Null,
    }
}

fn need_pick(app: &App) -> Result<(), agent_proto::RpcError> {
    match app.ui_screens.pick {
        Some(_) => Ok(()),
        None => Err(err(
            code::NO_DOCUMENT,
            "no screen is shown: ui.pick {\"screen\": ...}",
        )),
    }
}

pub fn call(app: &mut App, method: &str, p: &P) -> Reply {
    match method {
        m::UI_SCREENS => {
            app.sync_ui_disc();
            let retail = if app.ui_screens.has_disc() {
                app.ui_screens.retail()
            } else {
                Vec::new()
            };
            Ok(json!({
                "screens": UiScreens::tree().into_iter().map(|(s, states)| json!({"screen": s, "states": states})).collect::<Vec<_>>(),
                "retail": retail,
                "pick": pick_json(app.ui_screens.pick.as_ref()),
                "disc": app.ui_screens.has_disc(),
            }))
        }
        m::UI_PICK => {
            let pick = match (p.opt_str("screen")?, p.opt_str("retail")?) {
                (Some(screen), None) => {
                    let tree = UiScreens::tree();
                    let Some((_, states)) = tree.iter().find(|(n, _)| *n == screen) else {
                        return Err(err(
                            code::INVALID_PARAMS,
                            format!("no screen {screen:?} (ui.screens lists them)"),
                        ));
                    };
                    let state = match p.opt_str("state")? {
                        Some(s) if states.iter().any(|x| x == s) => s.to_owned(),
                        Some(s) => {
                            return Err(err(
                                code::INVALID_PARAMS,
                                format!("{screen} has no state {s:?} ({})", states.join(", ")),
                            ));
                        }
                        None => states.first().cloned().unwrap_or_default(),
                    };
                    Pick::Native {
                        screen: screen.to_owned(),
                        state,
                    }
                }
                (None, Some(tag)) => Pick::Retail(tag.to_owned()),
                _ => {
                    return Err(err(
                        code::INVALID_PARAMS,
                        "ui.pick takes screen (and state) or retail",
                    ));
                }
            };
            app.perform(Action::SetTool(ToolKind::UiScreens), false)
                .map_err(refused)?;
            app.perform(Action::UiPick(pick), false).map_err(refused)?;
            Ok(
                json!({ "pick": pick_json(app.ui_screens.pick.as_ref()), "summary": app.ui_screens.summary() }),
            )
        }
        m::UI_PRESS => {
            need_pick(app)?;
            let name = p.str("input")?;
            let input = Input::named(name).ok_or_else(|| {
                err(
                    code::INVALID_PARAMS,
                    format!("no input {name:?} (up down left right a b x y start back ...)"),
                )
            })?;
            let log = app
                .perform(Action::UiPress(input), false)
                .map_err(refused)?;
            Ok(json!({ "line": log.last(), "summary": app.ui_screens.summary() }))
        }
        m::UI_WIDGETS => {
            need_pick(app)?;
            if app.ui_screens.shot().is_none() {
                return Err(err(code::FAILED, app_last(app)));
            }
            let shot = app.ui_screens.shot().expect("drawn");
            Ok(json!({
                "widgets": shot.view.as_ref().map(|v| v.json()["widgets"].clone()).unwrap_or(json!([])),
                "findings": shot.findings.iter().map(|f| json!({"text": f.text, "ids": f.ids})).collect::<Vec<_>>(),
                "from_disc": shot.from_disc,
            }))
        }
        m::UI_RENDER => {
            need_pick(app)?;
            let out = match p.opt_str("out")? {
                Some(o) => PathBuf::from(o),
                None => {
                    let stem = match app.ui_screens.pick.as_ref().expect("picked") {
                        Pick::Native { screen, state } => format!("{screen}-{state}"),
                        Pick::Retail(t) => t.replace(['\\', '/'], "_"),
                    };
                    studio_proto::output_dir()
                        .join("ui")
                        .join(format!("{stem}.png"))
                }
            };
            super::docs::outside_git(&out)?;
            if let Some(dir) = out.parent() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| err(code::FAILED, format!("{}: {e}", dir.display())))?;
            }
            if app.ui_screens.shot().is_none() {
                return Err(err(code::FAILED, app_last(app)));
            }
            let shot = app.ui_screens.shot().expect("drawn");
            shot.canvas
                .write_png(&out)
                .map_err(|e| err(code::FAILED, format!("{e:#}")))?;
            Ok(json!({
                "path": out.display().to_string(),
                "width": shot.canvas.width,
                "height": shot.canvas.height,
                "findings": shot.findings.len(),
            }))
        }
        _ => Err(err(code::METHOD_NOT_FOUND, format!("no method {method}"))),
    }
}

fn app_last(app: &App) -> String {
    app.ui_screens
        .last
        .clone()
        .unwrap_or_else(|| "the screen did not draw".into())
}
