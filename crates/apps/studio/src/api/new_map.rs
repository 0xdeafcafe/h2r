//! `map.new`: File > New map over the API (docs/editor/agent-api.md, "New
//! maps"; docs/editor/editor.md, "New maps"). The build is the window's
//! (`App::start_new_map`); the call waits for it and answers what it wrote,
//! with the new map open.

use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::{MethodDef, P, Provider, Reply, def, err};
use crate::App;

pub struct NewMap;

impl Provider for NewMap {
    fn methods(&self) -> Vec<MethodDef> {
        vec![def(
            m::MAP_NEW,
            "File > New map: builds a starter map called `name` (`kind` multiplayer, the default, or campaign) over the open Halo 2 source's tags and writes it as a derived package in the data directory (maps/<map name>/): a lit courtyard floor with its lightmap and sky, walls, a ramp and cover of block-out pieces, and the kind's spawns (multiplayer: 16 respawns over two teams, CTF, assault, oddball, hill and territory points, a weapon and a vehicle; campaign: a start and one squad with its firing positions). Waits for the build, then opens the new map. REFUSED when no source is open, the source lacks the base map, the name is not a map name, a build is running, or the package exists (`replace` overwrites it); FAILED when the build fails, naming why",
            r#"{"name":"string (the map's title; its map name is lowercase letters, digits and '_')","kind?":"multiplayer|campaign","replace?":"boolean"}"#,
            r#"{"name":"string (the map name)","kind":"multiplayer|campaign","dir":"string (the package directory)","map_id":"string (maps.load's map_id)","scenario":"string","opened":"boolean"}"#,
            r#"{"name":"Courtyard","kind":"multiplayer"}"#,
        )]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        (method == m::MAP_NEW).then(|| new_map(app, &P(params)))
    }
}

fn new_map(app: &mut App, p: &P) -> Reply {
    let title = p.str("name")?;
    let kind = match p.opt_str("kind")? {
        None => starter_map::Kind::Multiplayer,
        Some(k) => starter_map::Kind::parse(k).map_err(|e| err(code::INVALID_PARAMS, e.to_string()))?,
    };
    let replace = p.bool("replace")?;
    app.start_new_map(title, kind, replace)
        .map_err(|e| err(code::REFUSED, e))?;
    app.wait_new_map();
    if let Some(e) = app.new_map_error() {
        let refused = e.contains("already exists") || e.contains("Git checkout");
        return Err(err(if refused { code::REFUSED } else { code::FAILED }, e.to_owned()));
    }
    let w = app
        .new_map_written()
        .ok_or_else(|| err(code::FAILED, "the build wrote nothing"))?
        .clone();
    let dir = w.dir.display().to_string();
    let opened = app.opened_package().is_some_and(|k| {
        w.dir
            .file_name()
            .is_some_and(|d| d.to_string_lossy() == k)
    });
    Ok(json!({
        "name": w.name,
        "kind": w.kind.name(),
        "dir": dir,
        "map_id": w.map_id,
        "scenario": w.scenario,
        "opened": opened,
    }))
}
