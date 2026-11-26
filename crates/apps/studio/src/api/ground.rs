//! `ground.status`, `ground.build` and `ground.save`: the AI ground of the open map
//! (docs/game/game-types-without-carriers.md, "In Studio";
//! docs/editor/agent-api.md, "AI ground"). Labels are placed with
//! `scenario.add {block: "label"}`; the built ground's sectors draw in the
//! Navigation layers and read through `nav.pathfinding`. `ground.save`
//! writes the labels to the edit target as a `map` item with a ground file.

use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::nav_mesh::sector_row;
use super::{MethodDef, P, Provider, Reply, def, err};
use crate::App;
use crate::app::ground::GroundState;

const STATUS_RESULT: &str = r#"{"state":"no_map|never|building|current|stale|failed","stale":"labels|collision|recipe|kit_version|null","job":"integer|null","error":"string|null","labels":"{ff_drop,ff_base,ff_cover,siege_site,siege_goal,siege_line,total}|null","built":"{zones,squads,sectors,links,problems:[{label,index,position,reason}],costs,headroom:{tag_growth,available,fits},bases}|null","shown":"boolean (the Navigation layers show the ground)","matching":"integer","sectors":"[{index,bsp,sector,flags,hint,floor,links,ring,center}]"}"#;

pub struct Ground;

impl Provider for Ground {
    fn methods(&self) -> Vec<MethodDef> {
        vec![
            def(
                m::GROUND_STATUS,
                "Where the open map's AI ground stands: never built, building, current, or stale (and why: labels, collision, recipe or kit_version), the labels placed by kind, and for a built ground its zones, squad slots, sectors, the labels it could not honour, its cost against the tag headroom and its bases. sectors lists the built ground's pathfinding sectors as nav.pathfinding does (offset and limit, 200)",
                r#"{"offset?":"integer","limit?":"integer"}"#,
                STATUS_RESULT,
                "{}",
            ),
            def(
                m::GROUND_BUILD,
                "Builds the AI ground over the open map as edited, with the labels placed (firefight_kit::ground::build), on its own thread, and answers at once with the job; with wait it answers when the build is done, with ground.status's result. A build already running is refused. The result draws in the Navigation layers and its problems go to problems.list (source ground)",
                r#"{"wait?":"boolean","offset?":"integer","limit?":"integer"}"#,
                r#"{"job":"integer","status":"ground.status's result"}"#,
                r#"{"wait":true}"#,
            ),
            def(
                m::GROUND_SAVE,
                "Writes the labels placed on the open map to the edit target as a `map` item (onto this map, served as `name`, default `<map>-ai`) with a ground file; the game builds the AI ground over it when it prepares its maps, so nothing is built here and the built ground is not saved. The edits the target holds for the open map are taken by the new map too. NO_TARGET with no edit target; REFUSED when the name is not a map name or is the open map's; NO_DOCUMENT with no map open",
                r#"{"name?":"string (the new map's name: letters, digits, '_' and '-')"}"#,
                r#"{"package":"string","name":"string","item":"integer","file":"string (the ground file in the package)","labels":"integer","changed":"boolean"}"#,
                r#"{"name":"example-ai"}"#,
            ),
        ]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        let p = P(params);
        Some(match method {
            m::GROUND_STATUS => status(app, &p),
            m::GROUND_BUILD => build(app, &p),
            m::GROUND_SAVE => save(app, &p),
            _ => return None,
        })
    }
}

fn status(app: &mut App, p: &P) -> Reply {
    let offset = p.opt_usize("offset")?.unwrap_or(0);
    let limit = p.opt_usize("limit")?.unwrap_or(200);
    app.poll_ground();
    Ok(status_json(app, offset, limit))
}

fn status_json(app: &mut App, offset: usize, limit: usize) -> Value {
    let mut v = app.ground_status();
    let (matching, rows) = match app.session.pathfinding() {
        Some(l) if app.session.shows_ground_pathfinding() => (
            l.sectors.len(),
            l.sectors
                .iter()
                .enumerate()
                .skip(offset)
                .take(limit)
                .map(|(i, s)| sector_row(i, s))
                .collect::<Vec<_>>(),
        ),
        _ => (0, Vec::new()),
    };
    v["matching"] = json!(matching);
    v["sectors"] = json!(rows);
    v
}

fn build(app: &mut App, p: &P) -> Reply {
    let wait = p.bool("wait")?;
    let offset = p.opt_usize("offset")?.unwrap_or(0);
    let limit = p.opt_usize("limit")?.unwrap_or(200);
    if app.ground_state() == GroundState::NoMap {
        return Err(err(code::NO_DOCUMENT, "no map is open"));
    }
    let job = app.start_ground().map_err(|e| {
        let refused = e.contains("already building");
        err(if refused { code::REFUSED } else { code::FAILED }, e)
    })?;
    if wait {
        app.wait_ground();
    }
    Ok(json!({"job": job, "status": status_json(app, offset, limit)}))
}

fn save(app: &mut App, p: &P) -> Reply {
    let name = p.opt_str("name")?;
    if app.ground_state() == GroundState::NoMap {
        return Err(err(code::NO_DOCUMENT, "no map is open"));
    }
    let saved = app.save_ground(name).map_err(|e| {
        let no_target = e == crate::app::edit_target::NO_TARGET;
        err(if no_target { code::NO_TARGET } else { code::REFUSED }, e)
    })?;
    Ok(json!({
        "package": saved.package,
        "name": saved.name,
        "item": saved.item,
        "file": saved.file,
        "labels": saved.labels,
        "changed": saved.changed,
    }))
}
