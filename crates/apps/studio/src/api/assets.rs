//! `asset.*`: the browser's rows, thumbnails and Copy into a package
//! (docs/editor/studio-redesign.md, "The browser"). The rows are
//! `app::assets`'s, the ones the browser panel reads.

use std::path::PathBuf;

use studio_proto::{code, method as m};

use super::{P, Reply, err};
use crate::App;
use crate::app::assets::{self, AssetId, CreateAsset, Query, Root, ThumbRefusal};
use crate::app::mods;
use crate::app::package_doc::{self, Refusal};
use ::workspace::SetLine;

/// The default row count of a list.
const DEFAULT_LIMIT: usize = 200;

fn asset(p: &P) -> Result<AssetId, agent_proto::RpcError> {
    AssetId::parse(p.str("asset")?).map_err(|e| err(code::INVALID_PARAMS, e))
}

fn list(app: &mut App, p: &P) -> Reply {
    let root = match p.opt_str("root")? {
        None => Root::Tags,
        Some(r) => Root::parse(r).ok_or_else(|| {
            err(
                code::INVALID_PARAMS,
                format!("root: {r:?} is not tags, packages or live"),
            )
        })?,
    };
    let q = Query {
        root,
        package: p.opt_str("package")?.map(str::to_owned),
        kind: p.opt_str("kind")?.map(str::to_owned),
        map: p.opt_str("map")?.map(str::to_owned),
        text: p.opt_str("query")?.unwrap_or("").to_owned(),
        limit: p.opt_usize("limit")?.unwrap_or(DEFAULT_LIMIT),
    };
    // The Voices kind lists voices, which are read from the data root.
    if root == Root::Tags && assets::is_voices_kind(q.kind.as_deref()) {
        return Ok(app.voice_asset_list(&q.text, q.limit));
    }
    let set = assets::read_set(&app.data_root());
    assets::list(&app.tags_root(), &set, &q).map_err(|e| err(code::REFUSED, e))
}

fn thumb(app: &mut App, p: &P) -> Reply {
    let id = asset(p)?;
    let set = assets::read_set(&app.data_root());
    let out = match p.opt_str("out")? {
        Some(o) => PathBuf::from(o),
        None => app.data_root().join("thumbs").join(format!(
            "{}.png",
            p.str("asset")?.replace([':', '\\', '/'], "_")
        )),
    };
    assets::thumb(&set, &id, &out).map_err(|e| match e {
        ThumbRefusal::NoPicture(m) => err(code::NOT_YET, m),
        ThumbRefusal::Failed(m) => err(code::FAILED, m),
    })
}

fn copy(app: &mut App, p: &P) -> Reply {
    let package = package_or_target(app, p)?;
    let request = CreateAsset::Copy {
        asset: p.str("asset")?.to_owned(),
        to: p.opt_str("to")?.map(str::to_owned),
    };
    app.create_asset(request, &package).map_err(super::import::copy_error)
}

fn links(app: &mut App, p: &P) -> Reply {
    let id = asset(p)?;
    let l = app.asset_links(&id).map_err(|e| err(code::REFUSED, e))?;
    Ok(l.json(p.str("asset")?))
}

/// `package`, else the edit target; with neither, `NO_TARGET`.
pub(super) fn package_or_target(app: &App, p: &P) -> Result<String, agent_proto::RpcError> {
    match p.opt_str("package")? {
        Some(id) => Ok(id.to_owned()),
        None => package_doc::target(&app.data_root()).ok_or_else(|| {
            err(
                code::NO_TARGET,
                "no package was given and there is no edit target: package.target {id} sets one",
            )
        }),
    }
}

fn refuse(r: Refusal) -> agent_proto::RpcError {
    match r {
        Refusal::Params(m) => err(code::INVALID_PARAMS, m),
        Refusal::Refused(m) => err(code::REFUSED, m),
        Refusal::Failed(m) => err(code::FAILED, m),
    }
}

/// `set`: a list of `{path, value, from?}` lines.
fn set_lines(p: &P) -> Result<Option<Vec<SetLine>>, agent_proto::RpcError> {
    let Some(v) = p.get("set") else {
        return Ok(None);
    };
    let list = v.as_array().ok_or_else(|| {
        err(
            code::INVALID_PARAMS,
            "set is a list of {path, value, from?} lines",
        )
    })?;
    let mut out = Vec::new();
    for (i, line) in list.iter().enumerate() {
        let path = line.get("path").and_then(serde_json::Value::as_str).ok_or_else(|| {
            err(code::INVALID_PARAMS, format!("set[{i}]: path is a string"))
        })?;
        let value = line.get("value").ok_or_else(|| {
            err(code::INVALID_PARAMS, format!("set[{i}]: value is required"))
        })?;
        let mut l = SetLine::from_json(path, value)
            .map_err(|e| err(code::INVALID_PARAMS, format!("set[{i}]: {e}")))?;
        if let Some(from) = line.get("from").filter(|f| !f.is_null()) {
            // The same value form as `value`: a line is made of `from` and
            // takes its value.
            let f = SetLine::from_json(path, from)
                .map_err(|e| err(code::INVALID_PARAMS, format!("set[{i}]: from: {e}")))?;
            l.from = Some(f.value);
        }
        out.push(l);
    }
    Ok(Some(out))
}

fn fork(app: &mut App, p: &P) -> Reply {
    let key = mods::parse_tag(p.str("tag")?).map_err(refuse)?;
    let map = p.str("map")?;
    let package = package_or_target(app, p)?;
    let set = set_lines(p)?;
    mods::fork(&app.data_root(), &package, &key, map, set).map_err(refuse)
}

fn revert(app: &mut App, p: &P) -> Reply {
    let key = mods::parse_tag(p.str("tag")?).map_err(refuse)?;
    let package = package_or_target(app, p)?;
    mods::revert(&app.data_root(), &package, &key, p.opt_str("map")?).map_err(refuse)
}

pub fn call(app: &mut App, method: &str, p: &P) -> Reply {
    match method {
        m::ASSET_FORK => fork(app, p),
        m::ASSET_REVERT => revert(app, p),
        m::ASSET_LIST => list(app, p),
        m::ASSET_THUMB => thumb(app, p),
        m::ASSET_COPY => copy(app, p),
        m::ASSET_LINKS => links(app, p),
        _ => Err(err(code::METHOD_NOT_FOUND, format!("no method {method}"))),
    }
}
