//! `package.*`: the package document (docs/editor/agent-api.md, "Packages";
//! docs/editor/studio-redesign.md, "Studio's side"). The reads and writes
//! are `app::package_doc`'s; this file is the wire: params, the edit target
//! as the default `id`, and the paired game for `package.play`.

use std::time::Duration;

use agent_proto::{CallError, Client, RpcError};
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::{P, Reply, err};
use crate::App;
use crate::app::assets::read_set;
use crate::app::mods;
use crate::app::package_doc::{self as doc, BaseMap, Refusal};

pub(super) fn refuse(r: Refusal) -> RpcError {
    match r {
        Refusal::Params(m) => err(code::INVALID_PARAMS, m),
        Refusal::Refused(m) => err(code::REFUSED, m),
        Refusal::Failed(m) => err(code::FAILED, m),
    }
}

/// Resolve the package for a save without opening the window's picker.
pub(super) fn save_target(app: &App, package: Option<&str>) -> Result<String, RpcError> {
    package.map(str::to_owned).or_else(|| app.edit_target()).ok_or_else(|| {
        err(code::NO_TARGET, crate::app::edit_target::NO_TARGET)
    })
}

/// `id`, else the edit target; with neither, `NO_TARGET`.
fn id_or_target(app: &App, p: &P) -> Result<String, RpcError> {
    match p.opt_str("id")? {
        Some(id) => Ok(id.to_owned()),
        None => doc::target(&app.data_root()).ok_or_else(|| {
            err(
                code::NO_TARGET,
                "no id was given and there is no edit target: package.target {id} sets one",
            )
        }),
    }
}

/// The maps a build starts from: `map` of the open source, or all of them.
fn bases(app: &App, map: Option<&str>) -> Result<Vec<BaseMap>, RpcError> {
    let source = app.source().ok_or_else(|| {
        err(
            code::NO_DOCUMENT,
            "no source is open: source.open takes a disc, a map or a folder of maps",
        )
    })?;
    let names: Vec<String> = match map {
        Some(m) => vec![m.to_owned()],
        None => app.maps().to_vec(),
    };
    let mut out = Vec::new();
    for name in names {
        match source.location(&name) {
            Ok((path, region)) => out.push(BaseMap { name, path, region }),
            Err(e) if map.is_some() => {
                return Err(err(code::INVALID_PARAMS, format!("map {name:?}: {e}")));
            }
            Err(_) => {}
        }
    }
    Ok(out)
}

fn value(p: &P) -> Value {
    p.0.get("value").cloned().unwrap_or(Value::Null)
}

fn build(app: &mut App, p: &P) -> Reply {
    let id = id_or_target(app, p)?;
    let map = p.opt_str("map")?;
    let bases = bases(app, map)?;
    let out = p.opt_str("out")?.map(std::path::PathBuf::from);
    let rows =
        doc::build(&app.data_root(), &id, &bases, map.is_none(), out.as_deref()).map_err(refuse)?;
    Ok(json!({ "id": id, "maps": rows }))
}

/// `package.diff`: the package's changes per asset, and with `map` the
/// conflict table's rows over that map. The rows are kept for Problems.
fn diff(app: &mut App, p: &P) -> Reply {
    let id = id_or_target(app, p)?;
    let map = p.opt_str("map")?;
    let bases = if app.source().is_some() {
        bases(app, map)?
    } else if map.is_some() {
        return Err(err(
            code::NO_DOCUMENT,
            "no source is open: source.open takes a disc, a map or a folder of maps",
        ));
    } else {
        Vec::new()
    };
    let (v, conflicts) = mods::diff(&app.data_root(), &id, &bases, map).map_err(refuse)?;
    if let Some(m) = map
        && v["built"] == true
    {
        app.problems.set_levels(m, conflicts);
    }
    Ok(v)
}

/// Build the map the package gives, then have the paired game load it. The
/// game reads its package set at boot: one that read another set (its
/// content hash differs) is refused, since loading would not show the
/// package.
fn play(app: &mut App, p: &P) -> Reply {
    let id = p.str("id")?;
    let map = p.str("map")?;
    let (endpoint, _) = super::preview::paired(app)?;
    let bases = bases(app, Some(map))?;
    let data = app.data_root();
    let mut rows = doc::build(&data, id, &bases, false, None).map_err(refuse)?;
    let row = rows
        .pop()
        .ok_or_else(|| err(code::FAILED, "no map was built"))?;
    let problems: Vec<String> = row["problems"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    if !problems.is_empty() {
        return Err(err(
            code::REFUSED,
            format!("{id} does not build {map}: {}", problems.join("; ")),
        ));
    }
    let failed = |what: &str, e: CallError| match e {
        CallError::Rpc(e) => err(code::FAILED, format!("{what}: {}", e.message)),
        e => err(code::FAILED, format!("{what}: {e}")),
    };
    let mut client = Client::connect_with_timeout(&endpoint, Some(Duration::from_secs(5)))
        .map_err(|e| {
            err(
                code::FAILED,
                format!("connecting to the paired game at {endpoint}: {e}"),
            )
        })?;
    client
        .set_timeout(Some(Duration::from_secs(60)))
        .map_err(|e| err(code::FAILED, format!("{endpoint}: {e}")))?;
    let ours = read_set(&data).content_hash.map(|h| h.to_string());
    if let Ok(theirs) = client.call(agent_proto::method::PACKAGES_LIST, Value::Null)
        && let (Some(theirs), Some(ours)) = (theirs["content_hash"].as_str(), ours.as_deref())
        && theirs != ours
    {
        return Err(err(
            code::REFUSED,
            format!(
                "the paired game read another package set at boot (content hash {theirs}, here {ours}): restart it, then play again"
            ),
        ));
    }
    let loaded = client
        .call(
            agent_proto::game_load::method::LOAD,
            json!({ "scenario": map }),
        )
        .map_err(|e| failed(agent_proto::game_load::method::LOAD, e))?;
    Ok(json!({ "id": id, "map": map, "build": row, "game": loaded }))
}

pub fn call(app: &mut App, method: &str, p: &P) -> Reply {
    let result = handle(app, method, p);
    if result.is_ok()
        && matches!(
            method,
            m::PACKAGE_NEW | m::PACKAGE_SET | m::PACKAGE_ITEM_ADD | m::PACKAGE_ITEM_SET
                | m::PACKAGE_ITEM_REMOVE | m::PACKAGE_TARGET | m::PACKAGE_ENABLE | m::PACKAGE_ORDER
                | m::PACKAGE_CHECK
        )
    {
        app.refresh_package_document();
        if method == m::PACKAGE_CHECK {
            app.refresh_problems(Some("package"));
        }
    }
    result
}

fn handle(app: &mut App, method: &str, p: &P) -> Reply {
    let data = app.data_root();
    match method {
        m::PACKAGE_LIST => Ok(doc::list(&data)),
        m::PACKAGE_READ => {
            doc::read_with(&data, p.str("id")?, Some(&app.tags_root())).map_err(refuse)
        }
        m::PACKAGE_NEW => doc::new(&data, p.str("id")?, p.str("name")?).map_err(refuse),
        m::PACKAGE_SET => doc::set(&data, p.str("id")?, p.str("path")?, &value(p)).map_err(refuse),
        m::PACKAGE_ITEM_ADD => {
            let id = id_or_target(app, p)?;
            let item = p.get("item").filter(|i| i.is_object()).ok_or_else(|| {
                err(
                    code::INVALID_PARAMS,
                    "item is an object: {kind, source?, inputs?, tag?, maps?}",
                )
            })?;
            doc::item_add(&data, &id, item, p.opt_usize("at")?).map_err(refuse)
        }
        m::PACKAGE_ITEM_SET => {
            let id = id_or_target(app, p)?;
            doc::item_set(&data, &id, p.usize("index")?, p.str("path")?, &value(p)).map_err(refuse)
        }
        m::PACKAGE_ITEM_REMOVE => {
            let id = id_or_target(app, p)?;
            doc::item_remove(&data, &id, p.usize("index")?).map_err(refuse)
        }
        m::PACKAGE_TARGET => {
            doc::edit_target(&data, p.opt_str("id")?, p.bool("clear")?).map_err(refuse)
        }
        m::PACKAGE_ENABLE => {
            let on = p
                .get("on")
                .and_then(Value::as_bool)
                .ok_or_else(|| err(code::INVALID_PARAMS, "on is true or false"))?;
            doc::enable(&data, p.str("id")?, on).map_err(refuse)
        }
        m::PACKAGE_ORDER => {
            let ids: Vec<String> = p
                .get("ids")
                .and_then(Value::as_array)
                .ok_or_else(|| err(code::INVALID_PARAMS, "ids is a list of package ids"))?
                .iter()
                .map(|v| {
                    v.as_str().map(str::to_owned).ok_or_else(|| {
                        err(
                            code::INVALID_PARAMS,
                            format!("ids: {v} is not a package id"),
                        )
                    })
                })
                .collect::<Result<_, _>>()?;
            doc::order(&data, &ids).map_err(refuse)
        }
        m::PACKAGE_CHECK => {
            let id = id_or_target(app, p)?;
            doc::check_with(&data, &id, Some(&app.tags_root())).map_err(refuse)
        }
        m::PACKAGE_DIFF => diff(app, p),
        m::PACKAGE_BUILD => build(app, p),
        m::PACKAGE_PLAY => play(app, p),
        m::PACKAGE_EXPORT => {
            let out = std::path::Path::new(p.str("out")?);
            super::docs::outside_git(out)?;
            doc::export(&data, &app.tags_root(), p.str("id")?, out).map_err(refuse)
        }
        _ => Err(err(code::METHOD_NOT_FOUND, format!("no method {method}"))),
    }
}
