//! `problems.list`: the Problems panel's list (`app::problems`), over
//! `check.run`'s checks, `ui.check`, `hud.check`, the packages' manifest
//! checks and the game's package load. docs/editor/agent-api.md.

use studio_proto::{code, method as m};

use super::{P, Reply, checks::CHECKS, err};
use crate::App;
use crate::app::problems::SOURCES;

fn list(app: &mut App, p: &P) -> Reply {
    let source = p.opt_str("source")?;
    if let Some(s) = source
        && !SOURCES.contains(&s)
    {
        return Err(err(
            code::INVALID_PARAMS,
            format!("source: {} (not {s:?})", SOURCES.join(", ")),
        ));
    }
    // `asset` narrows to the rows about one asset: what the header's
    // **Check** counts.
    let Some(asset) = p.opt_str("asset")? else {
        app.refresh_problems(source);
        return Ok(app.problems.json(source));
    };
    let narrowed = app
        .refresh_asset_problems(asset, source, CHECKS)
        .map_err(|e| err(code::INVALID_PARAMS, e))?;
    Ok(narrowed.json(source))
}

pub fn call(app: &mut App, method: &str, p: &P) -> Reply {
    match method {
        m::PROBLEMS_LIST => list(app, p),
        _ => Err(err(code::METHOD_NOT_FOUND, format!("no method {method}"))),
    }
}
