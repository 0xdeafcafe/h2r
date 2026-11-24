//! `asset.*`: the browser's rows, thumbnails and Copy into a package
//! (docs/editor/studio-redesign.md, "The browser"). The rows are
//! `app::assets`'s, the ones the browser panel reads.

use std::path::PathBuf;

use studio_proto::{code, method as m};

use super::{P, Reply, err};
use crate::App;
use crate::app::assets::{self, AssetId, CopyRefusal, Query, Root, ThumbRefusal};

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
    let id = asset(p)?;
    let package = p.str("package")?;
    app.asset_copy(&id, package, p.opt_str("to")?).map_err(|e| match e {
        CopyRefusal::Params(m) => err(code::INVALID_PARAMS, m),
        CopyRefusal::Refused(m) => err(code::REFUSED, m),
        CopyRefusal::Failed(m) => err(code::FAILED, m),
    })
}

fn links(app: &mut App, p: &P) -> Reply {
    let id = asset(p)?;
    let set = assets::read_set(&app.data_root());
    let l = crate::app::links::links(&app.tags_root(), &set, &id)
        .map_err(|e| err(code::REFUSED, e))?;
    Ok(l.json(p.str("asset")?))
}

pub fn call(app: &mut App, method: &str, p: &P) -> Reply {
    match method {
        m::ASSET_LIST => list(app, p),
        m::ASSET_THUMB => thumb(app, p),
        m::ASSET_COPY => copy(app, p),
        m::ASSET_LINKS => links(app, p),
        _ => Err(err(code::METHOD_NOT_FOUND, format!("no method {method}"))),
    }
}
