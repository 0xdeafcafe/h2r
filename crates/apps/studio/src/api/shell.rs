//! `shell.*`: the document tabs (docs/editor/studio-redesign.md, "The
//! shell"). Every call runs the window's own command
//! ([`Action::OpenDoc`], [`Action::FocusDoc`], [`Action::CloseDoc`]), so a
//! click on the tab strip and the call leave the same state. A tab's id
//! is what `doc.*` takes for it.

use agent_proto::RpcError;
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::scenario::refused;
use super::{P, Reply, err};
use crate::App;
use crate::app::{Action, Doc, Refused};

/// The tabs, in order, and the one in front.
pub fn tabs(app: &mut App) -> Value {
    app.sync_tabs();
    let front = app.front_doc().map(|d| app.doc_id(d));
    let tabs: Vec<Value> = app.tabs().iter().map(|t| app.tab_json(t)).collect();
    json!({ "front": front, "tool": super::tool_name(app.tool), "tabs": tabs })
}

/// The open tab a `doc` param names.
pub fn doc_param(app: &mut App, p: &P) -> Result<Doc, RpcError> {
    app.sync_tabs();
    let id = p.str("doc")?;
    app.find_doc(id).ok_or_else(|| {
        let open: Vec<String> = app.tabs().iter().map(|t| app.doc_id(t.doc)).collect();
        err(
            code::INVALID_PARAMS,
            format!(
                "no tab {id:?} is open (open: {})",
                if open.is_empty() {
                    "none".to_owned()
                } else {
                    open.join(", ")
                }
            ),
        )
    })
}

/// Runs a tab command; a failure is refused with the log's reason.
fn run(app: &mut App, action: Action, discard: bool) -> Result<Vec<String>, RpcError> {
    app.perform(action, discard).map_err(|r| match r {
        Refused::Failed(e) => err(code::REFUSED, e),
        other => refused(other),
    })
}

/// A tab's line once a command ran, with the log it wrote.
fn answer(app: &mut App, doc: Doc, log: Vec<String>) -> Reply {
    app.sync_tabs();
    let tab = app
        .tabs()
        .iter()
        .find(|t| t.doc == doc)
        .map(|t| app.tab_json(t))
        .unwrap_or(Value::Null);
    Ok(json!({ "tab": tab, "front": app.front_doc().map(|d| app.doc_id(d)), "log": log }))
}

/// `shell.open`: a tag of the open map (or of `map`, opened first), a
/// package item (`asset: "item:<package>:<n>"`), or a map alone.
fn open(app: &mut App, p: &P) -> Reply {
    let discard = p.bool("discard")?;
    let mut log = Vec::new();
    let map = p.opt_str("map")?;
    if let Some(map) = &map {
        if app.loaded_map().as_deref() != Some(map.as_str()) || app.browsing().is_some() {
            log.extend(run(app, Action::SelectMap(map.to_owned()), discard)?);
        }
        if app.loaded_map().as_deref() != Some(map.as_str()) {
            return Err(err(
                code::INVALID_PARAMS,
                format!("{map:?} did not open as the map (source.maps lists them)"),
            ));
        }
    }
    if let Some(asset) = p.opt_str("asset")? {
        if p.has("tag") {
            return Err(err(
                code::INVALID_PARAMS,
                "shell.open takes one of asset or tag",
            ));
        }
        // The way in the browser's double-click takes
        // (`Action::OpenAsset`), so both leave the same tab in front.
        log.extend(run(
            app,
            Action::OpenAsset {
                asset,
                map: map.clone(),
            },
            discard,
        )?);
        app.sync_tabs();
        let front = app.front_doc();
        let tab = front
            .and_then(|d| app.tabs().iter().find(|t| t.doc == d))
            .map(|t| app.tab_json(t));
        return Ok(json!({
            "tab": tab.unwrap_or(Value::Null),
            "front": front.map(|d| app.doc_id(d)),
            "log": log,
        }));
    }
    let doc = if p.has("tag") {
        if app.browsing().is_some() {
            return Err(err(
                code::REFUSED,
                "a browsed map's tags are read-only: shell.open {\"map\": ...} opens the map first",
            ));
        }
        Doc::Tag(super::tag_fields::resolve(app, p)?)
    } else if p.has("map") {
        Doc::Map
    } else {
        return Err(err(
            code::INVALID_PARAMS,
            "shell.open takes an asset, a tag, or a map",
        ));
    };
    log.extend(run(app, Action::OpenDoc(doc), false)?);
    // The scenario tag opens as the map's own tab.
    let shown = app.front_doc().unwrap_or(doc);
    answer(app, shown, log)
}

pub fn call(app: &mut App, method: &str, p: &P) -> Reply {
    match method {
        m::SHELL_TABS => Ok(tabs(app)),
        m::SHELL_OPEN => open(app, p),
        m::SHELL_FOCUS => {
            let doc = doc_param(app, p)?;
            let log = run(app, Action::FocusDoc(doc), false)?;
            answer(app, doc, log)
        }
        m::SHELL_CLOSE => {
            let doc = doc_param(app, p)?;
            let log = run(app, Action::CloseDoc(doc), p.bool("discard")?)?;
            app.sync_tabs();
            Ok(json!({
                "closed": !app.tabs().iter().any(|t| t.doc == doc),
                "front": app.front_doc().map(|d| app.doc_id(d)),
                "log": log,
            }))
        }
        _ => Err(err(code::METHOD_NOT_FOUND, format!("no method {method}"))),
    }
}
