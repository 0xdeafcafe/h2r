//! `ui.select_folder` and `ui.sheet`: the UI Screens tool's grid of
//! previews. `select_folder` shows the grid of a root or folder in the
//! editor (so `view.snapshot` of the window shows it); `sheet` writes the
//! same previews to one labelled contact-sheet PNG, CPU drawn, so an agent
//! sees what the grid shows. `ui.screens` lists the folders
//! ([`folders`]). docs/editor/agent-api.md "UI Screens".

use std::path::PathBuf;

use agent_proto::RpcError;
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::scenario::refused;
use super::ui_screens::pick_json;
use super::{P, Reply, err};
use crate::App;
use crate::app::ui_screens::browse::{self, Browse, Root};
use crate::app::ui_screens::gallery::Tile;
use crate::app::{Action, ToolKind};

pub fn call(app: &mut App, method: &str, p: &P) -> Reply {
    match method {
        m::UI_SELECT_FOLDER => select_folder(app, p),
        m::UI_SHEET => sheet(app, p),
        _ => Err(err(code::METHOD_NOT_FOUND, format!("no method {method}"))),
    }
}

fn browse_json(b: &Browse) -> Value {
    json!({ "root": b.root.name(), "folder": b.folder, "path": b.path() })
}

/// The folders of both trees and the grid shown, for `ui.screens`.
pub fn folders(app: &mut App) -> Value {
    let retail = app.ui_screens.retail_tree();
    json!({
        "ours": browse::ours_tree().tree.json(),
        "retail": retail.map(|r| {
            let mut j = r.tree.json();
            j["label"] = json!(Root::Retail.title());
            j["stripped"] = json!(r.stripped);
            j
        }),
        "browse": app.ui_screens.gallery.at.as_ref().map(browse_json),
    })
}

/// The root or folder the params name: `root` (`ours`, `retail`) and/or
/// `folder` (`ours/forge`, or with `root` given, `forge`). None when
/// neither is given.
fn target(p: &P) -> Result<Option<Browse>, RpcError> {
    let bad_root = |r: &str| err(code::INVALID_PARAMS, format!("root: {r:?} is not ours or retail"));
    Ok(match (p.opt_str("root")?, p.opt_str("folder")?) {
        (None, None) => None,
        (Some(r), None) => Some(Browse::root(Root::parse(r).ok_or_else(|| bad_root(r))?)),
        (None, Some(f)) => Some(Browse::parse(f).ok_or_else(|| {
            err(
                code::INVALID_PARAMS,
                format!("folder: {f:?} starts with neither ours/ nor retail/ (or give root)"),
            )
        })?),
        (Some(r), Some(f)) => Some(Browse {
            root: Root::parse(r).ok_or_else(|| bad_root(r))?,
            folder: f.trim_matches('/').to_owned(),
        }),
    })
}

/// `at` after checking it names a folder that is there.
fn checked(app: &mut App, at: Browse) -> Result<Browse, RpcError> {
    app.sync_ui_disc();
    if app.ui_screens.has_folder(&at) {
        return Ok(at);
    }
    if at.root == Root::Retail && !app.ui_screens.has_disc() {
        return Err(err(
            code::FAILED,
            "retail screens need the disc (source.open a disc image)",
        ));
    }
    Err(err(
        code::INVALID_PARAMS,
        format!("no folder {:?} (ui.screens lists them under folders)", at.path()),
    ))
}

fn select_folder(app: &mut App, p: &P) -> Reply {
    let Some(at) = target(p)? else {
        return Err(err(
            code::INVALID_PARAMS,
            "ui.select_folder takes folder (ours/<name>, retail/<path>) or root (ours, retail)",
        ));
    };
    let at = checked(app, at)?;
    app.perform(Action::SetTool(ToolKind::UiScreens), false)
        .map_err(refused)?;
    app.perform(Action::UiBrowse(Some(at.clone())), false)
        .map_err(refused)?;
    let states = app.ui_screens.gallery.states;
    let tiles: usize = app
        .ui_screens
        .sections(&at, states)
        .iter()
        .map(|s| s.tiles.len())
        .sum();
    Ok(json!({ "browse": browse_json(&at), "tiles": tiles, "states": states }))
}

fn sheet(app: &mut App, p: &P) -> Reply {
    let at = match target(p)? {
        Some(at) => at,
        None => app.ui_screens.gallery.at.clone().ok_or_else(|| {
            err(
                code::INVALID_PARAMS,
                "ui.sheet takes folder or root (or show a grid first: ui.select_folder)",
            )
        })?,
    };
    let at = checked(app, at)?;
    let states = if p.has("states") {
        p.bool("states")?
    } else {
        app.ui_screens.gallery.states
    };
    let columns = p.opt_usize("columns")?.unwrap_or(4).clamp(1, 12);
    let tiles: Vec<Tile> = app
        .ui_screens
        .sections(&at, states)
        .into_iter()
        .flat_map(|s| s.tiles)
        .collect();
    if tiles.is_empty() {
        return Err(err(code::FAILED, format!("nothing in {}", at.path())));
    }
    let out = match p.opt_str("out")? {
        Some(o) => PathBuf::from(o),
        None => studio_proto::output_dir().join("ui").join(format!(
            "sheet-{}{}.png",
            at.path().replace('/', "_"),
            if states { "-states" } else { "" }
        )),
    };
    super::docs::outside_git(&out)?;
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| err(code::FAILED, format!("{}: {e}", dir.display())))?;
    }
    let (canvas, placed) = app.ui_screens.sheet(&tiles, columns);
    canvas
        .write_png(&out)
        .map_err(|e| err(code::FAILED, format!("{e:#}")))?;
    let listed: Vec<Value> = tiles
        .iter()
        .zip(&placed)
        .map(|(t, s)| {
            let mut v = json!({
                "pick": pick_json(Some(&s.pick)),
                "name": t.name,
                "x": s.rect[0], "y": s.rect[1], "w": s.rect[2], "h": s.rect[3],
                "findings": s.findings,
                "found": s.found,
                "checked": s.checked,
            });
            if let Some(e) = &s.error {
                v["error"] = json!(e);
            }
            v
        })
        .collect();
    Ok(json!({
        "path": out.display().to_string(),
        "width": canvas.width,
        "height": canvas.height,
        "browse": browse_json(&at),
        "states": states,
        "tiles": listed,
    }))
}
