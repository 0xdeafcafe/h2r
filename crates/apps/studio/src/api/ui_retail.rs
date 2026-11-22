//! `ui.hide`, `ui.duplicate` and `ui.set_string`: a retail screen's tag
//! edits past a rect, as the agent API ([`super::Provider`], registered
//! in [`super::seams`]). Each is a write of the screen's `wgit` tag in
//! the open menu map's tag document (`app::ui_screens::edit::retail_tag`,
//! docs/game/ui-builder.md "Retail tag edits"): an undo step of the
//! window's, saved with the map as a patch by `ui.save`. They answer as
//! the other edits do ([`super::ui_edit`]); a screen of ours is refused.

use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::scenario::refused;
use super::ui_edit::{EDITED, def, edit, edited, need_pick};
use super::{MethodDef, P, Provider, Reply};
use crate::App;
use crate::app::Action;
use crate::app::ui_screens::edit::EditAction;

/// The retail tag edit provider.
#[derive(Default)]
pub struct UiRetail;

const METHODS: &[MethodDef] = &[
    def(
        m::UI_HIDE,
        "Hide a retail text or bitmap: park its block off screen, where the disc keeps its spare blocks, one undo step (a button, a list, a table and the title are refused with their owner; ui.revert shows it again)",
        r#"{"id":"string"}"#,
        r#"{"hidden":"string","widget":"null","findings":"[{text,ids}]","sink":"retail","dirty":"boolean"}"#,
        r#"{"id":"text.10"}"#,
    ),
    def(
        m::UI_DUPLICATE,
        "Copy a retail text, button or bitmap: append a copy of its block element to the end of its block (no other block changes its index), optionally at a rect, one undo step; the copy is selected. A full block grows only with Edit > Allow growing full blocks, which grow: true turns on",
        r#"{"id":"string","rect?":"[top,left,bottom,right]","grow?":"boolean"}"#,
        r#"{"copy":"string","of":"string","widget":"{id,rect,pixels,kind,...}","findings":"[{text,ids}]","sink":"retail","dirty":"boolean"}"#,
        r#"{"id":"text.10","rect":[95,-260,75,70],"grow":true}"#,
    ),
    def(
        m::UI_SET_STRING,
        "Set the string ID of a retail title, text or button to a string by name (lowercase letters, digits and _; added to the map when it has none), one undo step. shows is the stand-in text the picture draws for that string, which the game's code answers at run time (the tool's, not saved)",
        r#"{"id":"string","string":"string","shows?":"string"}"#,
        EDITED,
        r#"{"id":"text.10","string":"lobby_mode","shows":"CUSTOM GAMES"}"#,
    ),
];

impl Provider for UiRetail {
    fn methods(&self) -> Vec<MethodDef> {
        METHODS.to_vec()
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        if !METHODS.iter().any(|d| d.name == method) {
            return None;
        }
        Some(call(app, method, &P(params)))
    }
}

fn call(app: &mut App, method: &str, p: &P) -> Reply {
    need_pick(app)?;
    let id = p.str("id")?.to_owned();
    match method {
        m::UI_HIDE => {
            edit(app, EditAction::Hide { id: id.clone() })?;
            let mut reply = edited(app, &id)?;
            reply["hidden"] = json!(id);
            Ok(reply)
        }
        m::UI_DUPLICATE => {
            let rect = match p.get("rect") {
                Some(_) => Some(super::ui_edit::rect(p, "rect")?),
                None => None,
            };
            if p.get("grow").and_then(Value::as_bool) == Some(true) {
                app.perform(Action::SetAllowGrowth(true), false)
                    .map_err(refused)?;
            }
            edit(app, EditAction::Duplicate { id: id.clone() })?;
            let copy = app.ui_screens.edit.selected.clone().ok_or_else(|| {
                super::err(code::FAILED, format!("{id} was copied, and no copy is selected"))
            })?;
            if let Some(rect) = rect {
                edit(
                    app,
                    EditAction::SetRect {
                        id: copy.clone(),
                        rect,
                        merge: false,
                    },
                )?;
            }
            let mut reply = edited(app, &copy)?;
            reply["copy"] = json!(copy);
            reply["of"] = json!(id);
            Ok(reply)
        }
        m::UI_SET_STRING => {
            let string = p.str("string")?.to_owned();
            let shows = p.opt_str("shows")?.map(str::to_owned);
            edit(
                app,
                EditAction::SetString {
                    id: id.clone(),
                    string: string.clone(),
                    shows,
                },
            )?;
            let mut reply = edited(app, &id)?;
            reply["string"] = json!(string);
            Ok(reply)
        }
        _ => Err(super::err(
            code::METHOD_NOT_FOUND,
            format!("no method {method}"),
        )),
    }
}
