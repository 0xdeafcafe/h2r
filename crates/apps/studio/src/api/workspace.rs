//! `workspace.*` (docs/editor/agent-api.md, "The tag workspace"): the
//! tree's state, extraction and prune over `app::workspace`. The start
//! page (`ui/workspace.rs`) makes the same calls through the window's
//! buttons, so the API and the window are one path.
use std::path::PathBuf;

use serde_json::json;
use studio_proto::{code, method as m};

use super::{P, Reply, err};
use crate::app::App;
use crate::app::workspace::ExtractRefusal;

pub fn call(app: &mut App, method: &str, p: &P) -> Reply {
    match method {
        m::WORKSPACE_STATUS => Ok(app.workspace_status()),
        m::WORKSPACE_EXTRACT => {
            let disc = p.opt_str("disc")?.map(PathBuf::from);
            let maps = match p.get("maps") {
                None => None,
                Some(v) => Some(
                    v.as_array()
                        .and_then(|a| a.iter().map(|x| x.as_str().map(str::to_owned)).collect())
                        .ok_or_else(|| {
                            err(code::INVALID_PARAMS, "maps: a list of map names".to_owned())
                        })?,
                ),
            };
            let verify = p.bool("verify")?;
            let root = p.opt_str("root")?.map(PathBuf::from);
            let started = app
                .workspace_extract(disc, maps, verify, root)
                .map_err(|e| match e {
                    ExtractRefusal::Checkout { root, repo } => err(
                        code::REFUSED,
                        format!(
                            "{} is inside the Git checkout {}: the tags tree is game data and \
                             never sits in a repository; pick a root outside it",
                            root.display(),
                            repo.display()
                        ),
                    ),
                    ExtractRefusal::Busy => err(
                        code::REFUSED,
                        "an extraction is already running; workspace.status has its progress"
                            .to_owned(),
                    ),
                    ExtractRefusal::Params(m) => err(code::INVALID_PARAMS, m),
                })?;
            Ok(json!({
                "disc": started.disc.display().to_string(),
                "root": started.root.display().to_string(),
                "maps": started.maps,
                "verify": started.verify,
            }))
        }
        m::WORKSPACE_PRUNE => {
            let dry = p.bool("dry_run")?;
            app.workspace_prune(dry).map_err(|e| err(code::FAILED, e))
        }
        _ => Err(err(code::METHOD_NOT_FOUND, format!("no method {method}"))),
    }
}
