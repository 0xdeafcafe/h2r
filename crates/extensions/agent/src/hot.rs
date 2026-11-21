//! Hot iteration as host methods (docs/graphics/hot-iteration.md): live
//! tag edits (`edit.*`, over `hot-tags`) and the host renderer's overrides,
//! parameters and toggles (`render.*`, over `hot-render`'s hub). Names are
//! in `agent_proto::hot::method`.
//!
//! Like every host method, these run on the game's thread at a tick
//! boundary (or between slices), so a tag write lands between two ticks,
//! never inside one. The render methods only talk to the hub; the renderer
//! picks changes up at its next frame.

#[cfg(feature = "render-overrides")]
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent_proto::hot::method;
use agent_proto::sim::SessionView;
use agent_proto::{RpcError, code};
#[cfg(feature = "render-overrides")]
use hot_render::{DumpRequest, Hub};
use hot_tags::{Editor, ErrorKind, TagSel};
use runtime::Guest;
use serde_json::Value;
use serde_json::json;

use crate::{Access, Agent};

fn invalid(m: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, m)
}

fn edit_error(e: hot_tags::Error) -> RpcError {
    let c = match e.kind {
        ErrorKind::Invalid => code::INVALID_PARAMS,
        ErrorKind::Unavailable => code::UNAVAILABLE,
    };
    RpcError::new(c, e.message)
}

fn object(p: &Value) -> Result<&serde_json::Map<String, Value>, RpcError> {
    static EMPTY: std::sync::OnceLock<serde_json::Map<String, Value>> = std::sync::OnceLock::new();
    match p {
        Value::Null => Ok(EMPTY.get_or_init(serde_json::Map::new)),
        Value::Object(o) => Ok(o),
        _ => Err(invalid("params must be an object")),
    }
}

fn opt_str<'a>(
    o: &'a serde_json::Map<String, Value>,
    k: &str,
) -> Result<Option<&'a str>, RpcError> {
    match o.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(other) => Err(invalid(format!("{k}: {other} is not a string"))),
    }
}

fn limit(o: &serde_json::Map<String, Value>, default: usize) -> Result<usize, RpcError> {
    match o.get("limit") {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_u64()
            .map(|n| n as usize)
            .ok_or_else(|| invalid("limit: a whole number")),
    }
}

fn tag_sel(o: &serde_json::Map<String, Value>) -> Result<TagSel, RpcError> {
    Ok(TagSel {
        tag: opt_str(o, "tag")?
            .ok_or_else(|| {
                invalid("tag: a datum (\"0xe1230045\") or a name (edit.tags lists them)")
            })?
            .to_owned(),
        group: opt_str(o, "group")?.map(str::to_owned),
    })
}

#[cfg(feature = "render-overrides")]
fn to_json<T: serde::Serialize>(v: T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(|e| RpcError::new(code::UNAVAILABLE, e.to_string()))
}

/// Runs a function on the game's memory, locked for the call
/// (`|f| f(&mut runtime.memory())` in the game).
pub type WithGuest = Arc<dyn Fn(&mut dyn FnMut(&mut Guest)) + Send + Sync>;

fn with<T>(w: &WithGuest, f: impl FnOnce(&mut Guest) -> T) -> T {
    let mut f = Some(f);
    let mut out = None;
    w(&mut |g| {
        if let Some(f) = f.take() {
            out = Some(f(g));
        }
    });
    out.expect("the guest function ran")
}

fn permit(s: SessionView, p: &Value, epoch: u64) -> Result<(), RpcError> {
    if !s.writes {
        return Err(RpcError::new(
            code::INVALID_REQUEST,
            s.why.unwrap_or_default(),
        ));
    }
    if p.get("epoch").is_some_and(|v| v.as_u64() != Some(epoch)) {
        return Err(RpcError::new(
            code::INVALID_REQUEST,
            "the map epoch changed",
        ));
    }
    if s.kind == agent_proto::sim::SessionKind::MpHost
        && s.peers.unwrap_or(0) > 1
        && p["confirm_peers"].as_u64() != s.peers.map(u64::from)
    {
        return Err(RpcError::new(
            code::INVALID_REQUEST,
            "confirm the current peer count for host-only edits",
        ));
    }
    Ok(())
}

/// Adds `edit.*`: `editor` reads and writes the guest through `guest`.
pub fn add_edit_methods(
    agent: &Agent,
    editor: Editor,
    guest: WithGuest,
    session: impl Fn(&Guest) -> SessionView + Send + Sync + 'static,
    file: impl Fn(&str) -> Option<String> + Send + 'static,
    replaying: bool,
) {
    let session = Arc::new(move |g: &Guest, ctx: &crate::GameCtx<'_>| {
        let mut s = session(g).settle();
        let why = if replaying {
            Some("Theater playback")
        } else if ctx.live_session.is_some_and(|probe| probe(g)) {
            Some("external Live session")
        } else {
            None
        };
        if let Some(why) = why {
            s.writes = false;
            s.why = Some(why.into());
        }
        s
    });
    let editor = Arc::new(Mutex::new(editor));
    let lock = |e: &Arc<Mutex<Editor>>| -> Arc<Mutex<Editor>> { Arc::clone(e) };

    let (ed, g, probe) = (lock(&editor), guest.clone(), session.clone());
    let mut cached_file = None;
    agent.add_game_method(method::EDIT_MAP, Access::Read, move |p, ctx| {
        let mut ed = ed.lock().unwrap_or_else(|e| e.into_inner());
        ed.at_epoch(ctx.epoch);
        let (_, checksum) = ctx.reader.live_metadata();
        if cached_file.as_ref().is_none_or(|(epoch,_)| *epoch != ctx.epoch) {
            cached_file = Some((ctx.epoch,ctx.at.map.as_deref().and_then(&file)));
        }
        with(&g, |g| {
            let idx = hot_tags::index::index(g).map_err(|e| RpcError::new(code::UNAVAILABLE, e))?;
            let mut reply = json!({"at":ctx.at,"name":ctx.at.map,"file":cached_file.as_ref().and_then(|(_,f)| f.as_ref()),
                "checksum":checksum,"scenario":format!("{:#010x}",idx.scenario),"epoch":ctx.epoch,
                "session":probe(g,ctx)});
            if p.get("known_epoch").is_some_and(|e| e.as_u64() != Some(ctx.epoch)) {
                let entries = hot_tags::index::all(g,&idx).map_err(|e| RpcError::new(code::UNAVAILABLE,e))?;
                reply["loaded"] = json!(entries.into_iter().map(|e| e.datum).collect::<Vec<_>>());
            }
            Ok(reply)
        })
    });
    let (ed, g, probe) = (lock(&editor), guest.clone(), session.clone());
    agent.add_game_method(method::EDIT_BATCH, Access::Act, move |p, ctx| {
        let mut ed = ed.lock().unwrap_or_else(|e| e.into_inner());
        ed.at_epoch(ctx.epoch);
        with(&g, |g| {
            permit(probe(g, ctx), &p, ctx.epoch)?;
            let writes = p["writes"]
                .as_array()
                .ok_or_else(|| invalid("writes must be an array"))?;
            ed.batch(g, writes, opt_str(object(&p)?, "label")?)
                .map_err(|e| match e {
                    hot_tags::BatchError::Field(e) => edit_error(e),
                    hot_tags::BatchError::Mismatch(fields) => RpcError {
                        code: code::MISMATCH,
                        message: "the game changed these fields".into(),
                        data: Some(json!({"fields":fields})),
                    },
                })
        })
    });
    let (ed, g) = (lock(&editor), Arc::clone(&guest));
    agent.add_method(method::EDIT_TAGS, Access::Read, move |p| {
        let o = object(&p)?;
        let (group, name, limit) = (opt_str(o, "group")?, opt_str(o, "name")?, limit(o, 200)?);
        let mut ed = ed.lock().unwrap_or_else(|e| e.into_inner());
        with(&g, |g| ed.tags(g, group, name, limit)).map_err(edit_error)
    });
    let (ed, g) = (lock(&editor), Arc::clone(&guest));
    agent.add_method(method::EDIT_GET, Access::Read, move |p| {
        let o = object(&p)?;
        let sel = tag_sel(o)?;
        let path = opt_str(o, "path")?.unwrap_or("");
        let mut ed = ed.lock().unwrap_or_else(|e| e.into_inner());
        with(&g, |g| ed.get(g, &sel, path)).map_err(edit_error)
    });
    let (ed, g) = (lock(&editor), Arc::clone(&guest));
    let probe = session.clone();
    agent.add_game_method(method::EDIT_SET, Access::Act, move |p, ctx| {
        let o = object(&p)?;
        let sel = tag_sel(o)?;
        let path = opt_str(o, "path")?.ok_or_else(|| invalid("path: the field to set"))?;
        let value = o
            .get("value")
            .ok_or_else(|| invalid("value: what to set it to"))?;
        let mut ed = ed.lock().unwrap_or_else(|e| e.into_inner());
        ed.at_epoch(ctx.epoch);
        with(&g, |g| {
            permit(probe(g, ctx), &p, ctx.epoch)?;
            ed.set(g, &sel, path, value).map_err(edit_error)
        })
    });
    let (ed, g) = (lock(&editor), Arc::clone(&guest));
    agent.add_game_method(method::EDIT_CHANGES, Access::Read, move |_, ctx| {
        let mut ed = ed.lock().unwrap_or_else(|e| e.into_inner());
        ed.at_epoch(ctx.epoch);
        Ok(with(&g, |g| ed.changes(g)))
    });
    let (ed, g) = (lock(&editor), guest);
    agent.add_game_method(method::EDIT_REVERT, Access::Act, move |p, ctx| {
        let o = object(&p)?;
        let id = match o.get("id") {
            None | Some(Value::Null) => None,
            Some(v) => Some(
                v.as_u64()
                    .ok_or_else(|| invalid("id: a change id (edit.changes)"))?,
            ),
        };
        let mut ed = ed.lock().unwrap_or_else(|e| e.into_inner());
        ed.at_epoch(ctx.epoch);
        with(&g, |g| {
            permit(session(g, ctx), &p, ctx.epoch)?;
            ed.revert(g, id).map_err(edit_error)
        })
    });
}

#[cfg(feature = "render-overrides")]
/// Adds `render.*` over `hub` (`hot_render::global()` in the game).
pub fn add_render_methods(agent: &Agent, hub: &'static Hub) {
    agent.add_method(method::RENDER_OVERRIDES, Access::Read, move |_| {
        to_json(hub.status())
    });
    agent.add_method(method::RENDER_RELOAD, Access::Act, move |p| {
        let o = object(&p)?;
        if let Some(dir) = opt_str(o, "dir")? {
            let dir = PathBuf::from(dir);
            std::fs::create_dir_all(&dir)
                .map_err(|e| invalid(format!("{}: {e}", dir.display())))?;
            hub.set_dir(Some(dir));
        }
        to_json(hub.reload())
    });
    agent.add_method(method::RENDER_PROGRAMS, Access::Read, move |p| {
        let o = object(&p)?;
        let vertex = opt_str(o, "vertex")?;
        let limit = limit(o, 500)?;
        let status = hub.status();
        let mut total = 0;
        let mut out = Vec::new();
        for (id, info) in hub.programs() {
            if vertex.is_some_and(|v| v != info.vertex) {
                continue;
            }
            total += 1;
            if out.len() < limit {
                let o = status.overrides.iter().find(|o| o.program == id);
                let mut v = to_json(&info)?;
                v["program"] = json!(id);
                v["override"] = match o {
                    Some(o) => to_json(&o.applied)?,
                    None => Value::Null,
                };
                out.push(v);
            }
        }
        Ok(json!({ "total": total, "programs": out }))
    });
    agent.add_method(method::RENDER_DUMP, Access::Act, move |p| {
        let o = object(&p)?;
        let program = match opt_str(o, "program")? {
            None | Some("all") => None,
            Some(s) => {
                let id = hot_render::parse_program(s)
                    .ok_or_else(|| invalid(format!("program: {s:?} is not 16 hex digits")))?;
                if !hub.has_program(id) {
                    return Err(invalid(format!(
                        "program {s}: not translated (render.programs lists them)"
                    )));
                }
                Some(id)
            }
        };
        let to = match opt_str(o, "to")? {
            Some(d) => PathBuf::from(d),
            None => hub
                .dir()
                .map(|d| d.join("translated"))
                .ok_or_else(|| invalid("to: where to write (no override directory is set)"))?,
        };
        hub.request_dump(DumpRequest {
            program,
            to: to.clone(),
            tries: 0,
        });
        Ok(json!({
            "queued": true,
            "program": program.map(hot_render::program_text),
            "to": to.display().to_string(),
            "note": "written at the next frame; render.overrides shows last_dump",
        }))
    });
    agent.add_method(method::RENDER_PARAM, Access::Act, move |p| {
        let o = object(&p)?;
        let Some(name) = opt_str(o, "name")? else {
            return to_json(json!({ "params": hub.params() }));
        };
        match o.get("value") {
            None => hub
                .params()
                .into_iter()
                .find(|p| p.name == name)
                .ok_or_else(|| invalid(format!("no parameter {name:?}")))
                .and_then(to_json),
            Some(Value::Null) => hub.set_param(name, None).map_err(invalid).and_then(to_json),
            Some(v) => {
                let v = v
                    .as_f64()
                    .ok_or_else(|| invalid("value: a number (or null to reset)"))?;
                hub.set_param(name, Some(v as f32))
                    .map_err(invalid)
                    .and_then(to_json)
            }
        }
    });
    agent.add_method(method::RENDER_TOGGLE, Access::Act, move |p| {
        let o = object(&p)?;
        let Some(name) = opt_str(o, "name")? else {
            return to_json(json!({ "toggles": hub.toggles() }));
        };
        match o.get("on") {
            None => hub
                .toggles()
                .into_iter()
                .find(|t| t.name == name)
                .ok_or_else(|| invalid(format!("no toggle {name:?}")))
                .and_then(to_json),
            Some(Value::Null) => hub
                .set_toggle(name, None)
                .map_err(invalid)
                .and_then(to_json),
            Some(Value::Bool(on)) => hub
                .set_toggle(name, Some(*on))
                .map_err(invalid)
                .and_then(to_json),
            Some(other) => Err(invalid(format!("on: {other} is not true, false or null"))),
        }
    });
}

#[cfg(feature = "render-overrides")]
/// Points `hub` at `dir`, loads it and watches it (a quarter-second look).
/// Returns the first load's report.
pub fn watch_overrides(hub: &'static Hub, dir: PathBuf) -> hot_render::ReloadReport {
    let _ = std::fs::create_dir_all(&dir);
    hub.set_dir(Some(dir));
    let r = hub.reload();
    if let Err(e) = hot_render::watch(hub, std::time::Duration::from_millis(250)) {
        eprintln!("render overrides: not watching: {e}");
    }
    r
}

#[cfg(test)]
mod tests;
