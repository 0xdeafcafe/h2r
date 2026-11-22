//! `ui.select`, `ui.move`, `ui.set_rect`, `ui.set_text`, `ui.revert`,
//! `ui.undo`, `ui.redo`, `ui.edits`, `ui.save` and `ui.animate`: the UI
//! Screens tool's editing as the agent API ([`super::Provider`], registered
//! in [`super::seams`]). Edits go through `Action::UiEdit`, so an agent's
//! edit is the window's undo step and save (docs/game/ui-builder.md
//! "Editing in the editor", docs/editor/agent-api.md). Each edit answers
//! with the widget as it is now and the checks' findings on the edited
//! view; `ui.render` after it draws it.

use std::path::PathBuf;

use boot::ui_kit::animation::Transition;
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::scenario::refused;
use super::{MethodDef, P, Provider, Reply, err};
use crate::App;
use crate::app::ui_screens::edit::{EditAction, Sink};
use crate::app::{Action, Pick, ToolKind};

/// The UI Screens editing provider.
#[derive(Default)]
pub struct UiEdit;

const fn def(
    name: &'static str,
    summary: &'static str,
    params: &'static str,
    result: &'static str,
    example: &'static str,
) -> MethodDef {
    MethodDef {
        name,
        summary,
        params,
        result,
        example,
        planned: None,
    }
}

const EDITED: &str = r#"{"widget":"{id,rect,pixels,kind,...}","findings":"[{text,ids}]","sink":"ours|retail","dirty":"boolean"}"#;

/// Rects are `[top, left, bottom, right]` in UI units (640 by 480, origin
/// at the centre, Y up), as `ui.widgets` gives them.
const METHODS: &[MethodDef] = &[
    def(
        m::UI_SELECT,
        "Select a widget of the screen shown by id (null or none: nothing); its JSON",
        r#"{"id?":"string|null"}"#,
        r#"{"selected":"string|null","widget":"object|null"}"#,
        r#"{"id":"title"}"#,
    ),
    def(
        m::UI_MOVE,
        "Move a widget by dx, dy UI units (Y up), one undo step: our screens set `at` in the screen's YAML, retail screens a field of the mainmenu tag",
        r#"{"id":"string","dx":"integer","dy":"integer"}"#,
        EDITED,
        r#"{"id":"title","dx":4,"dy":-2}"#,
    ),
    def(
        m::UI_SET_RECT,
        "Give a widget a rect [top, left, bottom, right] in UI units, one undo step (our screens: `at` and `size`; retail: the block's rect, or its point for art, lists and tables)",
        r#"{"id":"string","rect":"[top,left,bottom,right]"}"#,
        EDITED,
        r#"{"id":"title","rect":[200,-260,176,60]}"#,
    ),
    def(
        m::UI_SET_TEXT,
        "Give a static text a new text (our screens: `text` in the YAML, its {slots} kept); dynamic values and retail strings are refused with their owner",
        r#"{"id":"string","text":"string"}"#,
        EDITED,
        r#"{"id":"title","text":"Edited"}"#,
    ),
    def(
        m::UI_REVERT,
        "Put a widget (or, without id, the whole screen) back as last loaded or saved, one undo step",
        r#"{"id?":"string"}"#,
        r#"{"line":"string","edits":"object"}"#,
        r#"{"id":"title"}"#,
    ),
    def(
        m::UI_UNDO,
        "Undo the UI Screens tool's last edit (our screens' history, or the map's for a retail screen)",
        "{}",
        r#"{"done":"boolean","can_undo":"boolean","can_redo":"boolean","edits":"object"}"#,
        "null",
    ),
    def(
        m::UI_REDO,
        "Redo the UI Screens tool's last undone edit",
        "{}",
        r#"{"done":"boolean","can_undo":"boolean","can_redo":"boolean","edits":"object"}"#,
        "null",
    ),
    def(
        m::UI_EDITS,
        "The edits: the sink of the screen shown, each edited screen's file and YAML, the retail widgets moved, what is unsaved, the checkout",
        "{}",
        r#"{"sink":"ours|retail|null","selected":"string|null","ours":"[{screen,file,yaml}]","ours_dirty":"boolean","retail":"{tag: {id: rect}}","retail_dirty":"boolean","checkout":"string|null","changed_on_disk":"[string]"}"#,
        "null",
    ),
    def(
        m::UI_SAVE,
        "Save the edits: our screens' YAML into the checkout's assets/ui/screens (refused outside a checkout); a retail screen's with the map, as doc.save does",
        "{}",
        r#"{"sink":"ours|retail","written?":"[path]","save?":"doc.save result"}"#,
        "null",
    ),
    def(
        m::UI_ANIMATE,
        "Write a filmstrip PNG of the screen shown's transition (open or close): frames side by side, each labelled with its time (default 5 frames; never into a Git checkout)",
        r#"{"phase":"string","frames?":"integer (1..32)","out?":"string"}"#,
        r#"{"path":"string","width":"integer","height":"integer","frames":"[{t,tick,x,w}]"}"#,
        r#"{"phase":"open","frames":4,"out":"$OUT/open.png"}"#,
    ),
];

impl Provider for UiEdit {
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

fn need_pick(app: &App) -> Result<Sink, agent_proto::RpcError> {
    app.ui_screens.pick.as_ref().map(Sink::of).ok_or_else(|| {
        err(
            code::NO_DOCUMENT,
            "no screen is shown: ui.pick {\"screen\": ...}",
        )
    })
}

/// Runs `a`; its refusal or failure as the API's error.
fn edit(app: &mut App, a: EditAction) -> Result<Vec<String>, agent_proto::RpcError> {
    app.perform(Action::SetTool(ToolKind::UiScreens), false)
        .map_err(refused)?;
    app.perform(Action::UiEdit(a), false).map_err(refused)
}

fn findings(app: &mut App) -> Value {
    match app.ui_screens.shot() {
        Some(shot) => Value::Array(
            shot.findings
                .iter()
                .map(|f| json!({"text": f.text, "ids": f.ids}))
                .collect(),
        ),
        None => json!([]),
    }
}

/// What an edit answers: the widget now, the findings, the sink, unsaved.
fn edited(app: &mut App, id: &str) -> Reply {
    let sink = need_pick(app)?;
    let widget = app.ui_screens.edit_widget_json(id).unwrap_or(Value::Null);
    let dirty = match sink {
        Sink::Ours => app.ui_screens.edit.ours.dirty(),
        Sink::Retail => app.session.is_dirty(),
    };
    Ok(json!({
        "widget": widget,
        "findings": findings(app),
        "sink": sink.name(),
        "dirty": dirty,
    }))
}

fn rect(p: &P, k: &str) -> Result<[i16; 4], agent_proto::RpcError> {
    let bad = || {
        err(
            code::INVALID_PARAMS,
            format!("{k}: expected [top, left, bottom, right] in whole UI units"),
        )
    };
    let v = p.get(k).ok_or_else(bad)?;
    let a = v.as_array().filter(|a| a.len() == 4).ok_or_else(bad)?;
    let mut out = [0i16; 4];
    for (o, x) in out.iter_mut().zip(a) {
        *o = x
            .as_i64()
            .and_then(|n| i16::try_from(n).ok())
            .ok_or_else(bad)?;
    }
    Ok(out)
}

fn unit(p: &P, k: &str) -> Result<i16, agent_proto::RpcError> {
    let n = p.i64(k)?;
    i16::try_from(n).map_err(|_| err(code::INVALID_PARAMS, format!("{k}: {n} is out of range")))
}

fn call(app: &mut App, method: &str, p: &P) -> Reply {
    if method == m::UI_EDITS {
        return Ok(app.ui_edits_json());
    }
    let sink = need_pick(app)?;
    match method {
        m::UI_SELECT => {
            let id = p.opt_str("id")?.map(str::to_owned);
            edit(app, EditAction::Select(id.clone()))?;
            let widget = match &id {
                Some(id) => app.ui_screens.edit_widget_json(id).unwrap_or(Value::Null),
                None => Value::Null,
            };
            Ok(json!({ "selected": app.ui_screens.edit.selected, "widget": widget }))
        }
        m::UI_MOVE => {
            let id = p.str("id")?.to_owned();
            let (dx, dy) = (unit(p, "dx")?, unit(p, "dy")?);
            edit(
                app,
                EditAction::Move {
                    id: id.clone(),
                    dx,
                    dy,
                    merge: false,
                },
            )?;
            edited(app, &id)
        }
        m::UI_SET_RECT => {
            let id = p.str("id")?.to_owned();
            let rect = rect(p, "rect")?;
            edit(
                app,
                EditAction::SetRect {
                    id: id.clone(),
                    rect,
                    merge: false,
                },
            )?;
            edited(app, &id)
        }
        m::UI_SET_TEXT => {
            let id = p.str("id")?.to_owned();
            let text = p.str("text")?.to_owned();
            edit(app, EditAction::SetText { id: id.clone(), text })?;
            edited(app, &id)
        }
        m::UI_REVERT => {
            let id = p.opt_str("id")?.map(str::to_owned);
            let log = edit(app, EditAction::Revert(id))?;
            Ok(json!({ "line": log.last(), "edits": app.ui_edits_json() }))
        }
        m::UI_UNDO | m::UI_REDO => {
            let undo = method == m::UI_UNDO;
            app.perform(Action::SetTool(ToolKind::UiScreens), false)
                .map_err(refused)?;
            let done = app.ui_can_undo_redo(!undo);
            let action = if undo {
                Action::UndoIn(ToolKind::UiScreens)
            } else {
                Action::RedoIn(ToolKind::UiScreens)
            };
            app.perform(action, false).map_err(refused)?;
            Ok(json!({
                "done": done,
                "can_undo": app.ui_can_undo_redo(false),
                "can_redo": app.ui_can_undo_redo(true),
                "edits": app.ui_edits_json(),
            }))
        }
        m::UI_SAVE => match sink {
            Sink::Ours => {
                let dirty = app.ui_screens.edit.ours.dirty_screens();
                let checkout = app.ui_screens.edit.ours.checkout_path().map(PathBuf::from);
                edit(app, EditAction::Save)?;
                let written: Vec<String> = match checkout {
                    Some(c) => dirty
                        .iter()
                        .map(|s| {
                            crate::app::ui_screens::edit::ours::file_of(&c, s)
                                .display()
                                .to_string()
                        })
                        .collect(),
                    None => Vec::new(),
                };
                Ok(json!({ "sink": "ours", "written": written }))
            }
            Sink::Retail => {
                let save = super::docs::call(app, m::DOC_SAVE, &P(&json!({"doc": "map"})))?;
                Ok(json!({ "sink": "retail", "save": save }))
            }
        },
        m::UI_ANIMATE => {
            let name = p.str("phase")?;
            let t = Transition::named(name).ok_or_else(|| {
                err(
                    code::INVALID_PARAMS,
                    format!("phase: open or close, not {name:?}"),
                )
            })?;
            let n = p.opt_usize("frames")?.unwrap_or(5);
            let out = match p.opt_str("out")? {
                Some(o) => PathBuf::from(o),
                None => {
                    let stem = match app.ui_screens.pick.as_ref().expect("picked") {
                        Pick::Native { screen, state } => format!("{screen}-{state}"),
                        Pick::Retail(tag) => tag.replace(['\\', '/'], "_"),
                    };
                    studio_proto::output_dir()
                        .join("ui")
                        .join(format!("{stem}-{}.png", t.name()))
                }
            };
            super::docs::outside_git(&out)?;
            let (strip, frames) = app
                .ui_screens
                .filmstrip(t, n)
                .map_err(|e| err(code::INVALID_PARAMS, format!("{e:#}")))?;
            if let Some(dir) = out.parent() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| err(code::FAILED, format!("{}: {e}", dir.display())))?;
            }
            strip
                .write_png(&out)
                .map_err(|e| err(code::FAILED, format!("{e:#}")))?;
            Ok(json!({
                "path": out.display().to_string(),
                "width": strip.width,
                "height": strip.height,
                "frames": frames
                    .iter()
                    .map(|f| json!({"t": f.seconds, "tick": f.tick, "x": f.x, "w": f.w}))
                    .collect::<Vec<_>>(),
            }))
        }
        _ => Err(err(code::METHOD_NOT_FOUND, format!("no method {method}"))),
    }
}
