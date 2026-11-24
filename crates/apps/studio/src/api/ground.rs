//! `ground.status` and `ground.build`: the AI ground of the open map
//! (docs/game/game-types-without-carriers.md, "In Studio";
//! docs/editor/agent-api.md, "AI ground"). Labels are placed with
//! `scenario.add {block: "label"}`; the built ground's sectors draw in the
//! Navigation layers and read through `nav.pathfinding`.

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
        ]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        let p = P(params);
        Some(match method {
            m::GROUND_STATUS => status(app, &p),
            m::GROUND_BUILD => build(app, &p),
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
