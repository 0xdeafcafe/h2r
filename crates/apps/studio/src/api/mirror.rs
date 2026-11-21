//! The viewport's current mirror and the exact game samples it interpolates.
use agent_proto::sim::objects::handle;
use guest_layout::ObjectType;
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::{MethodDef, P, Provider, Reply, def, err};
use crate::App;
use crate::play::{
    Play,
    pair::Request,
    presence::{CameraMode, PlayerMove},
    select,
};
use scenario_edit::geometry::Ray;

pub struct Mirror;

impl Provider for Mirror {
    fn methods(&self) -> Vec<MethodDef> {
        vec![
            def(
                m::MIRROR_STATUS,
                "The paired game's stream, its latest exact tick and the sample shown in the viewport",
                "{}",
                r#"{"stream":"{state,stream,protocol,layers,tick_rate,fps,delay_ms,dropped,skipped,notice}","store":"{tick,first_tick,shown_tick,objects,bytes}","mode":"view|edit","why":"string|null"}"#,
                "null",
            ),
            def(
                m::MIRROR_PENDING,
                "Document edits waiting for the game and its existing journal",
                "{}",
                r#"{"pending":"[edit]","journal":"[change]","leaving":"boolean"}"#,
                "null",
            ),
            def(
                m::MIRROR_APPLY,
                "Send, resolve a conflict, adopt or revert the game journal, respawn or reload",
                r#"{"what":"string","at?":"integer","peers?":"integer"}"#,
                r#"{"queued":"string"}"#,
                r#"{"what":"send"}"#,
            ),
            def(
                m::MIRROR_PICK,
                "Pick the first live object before the level surface, or null on a miss, without changing selection",
                r#"{"from?":"aim","ray?":{"origin":"[x,y,z]","dir":"[x,y,z]"}}"#,
                r#"[{"object":"handle","placement":"entry|null","tag":"handle|null","point":"[x,y,z]","distance":"number"},null]"#,
                r#"{"from":"aim"}"#,
            ),
            def(
                m::MIRROR_AIM_SELECT,
                "Read or toggle selection by dwelling on a live object with the local player's aim",
                r#"{"enabled?":"boolean"}"#,
                r#"{"enabled":"boolean","candidate":"hit|null"}"#,
                "{}",
            ),
            def(
                m::MIRROR_CAMERA,
                "Read or set the editor viewport's camera mode for the local player",
                r#"{"mode?":"free|follow|eyes","target?":"player.<index>"}"#,
                r#"{"mode":"free|follow|eyes","target":"player.<index>|null"}"#,
                "{}",
            ),
            def(
                m::MIRROR_PLAYER,
                "Move the local player in Edit mode, optionally matching the editor camera's facing",
                r#"{"to":"cursor|camera|[x,y,z]","facing?":"boolean"}"#,
                r#"{"requested":"boolean"}"#,
                r#"{"to":"camera","facing":true}"#,
            ),
            def(
                m::MIRROR_OBJECTS,
                "Objects at the shown tick, or an exact retained game tick. Handles, kinds and parent-relative positions use sim.objects conventions",
                r#"{"at_tick?":"integer","kind?":"tag group","near?":{"point":"[x,y,z]","radius":"number"},"limit?":"integer"}"#,
                r#"{"tick":"integer|null","objects":"[{id,kind,tag,pos,forward,up,scale,velocity,flags,parent,body,shield,placement,nodes}]","truncated":"boolean"}"#,
                r#"{"limit":4}"#,
            ),
        ]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        match method {
            m::MIRROR_STATUS => Some(Ok(status(app))),
            m::MIRROR_PENDING => Some(Ok(
                json!({"pending":app.pairing.pending,"journal":app.pairing.journal,"leaving":app.pairing.leaving}),
            )),
            m::MIRROR_APPLY => Some(apply(app, &P(params))),
            m::MIRROR_PICK => Some(pick(app, &P(params))),
            m::MIRROR_AIM_SELECT => Some(aim_select(app, &P(params))),
            m::MIRROR_CAMERA => Some(camera(app, &P(params))),
            m::MIRROR_PLAYER => {
                let result = move_player(app, &P(params));
                if let Err(error) = &result
                    && error.code == code::REFUSED
                    && app.pairing.published.is_some()
                {
                    app.pairing
                        .requests
                        .push(Request::PlayerMoveRefused(error.message.clone()));
                }
                Some(result)
            }
            m::MIRROR_OBJECTS => Some(objects(app, &P(params))),
            _ => None,
        }
    }
}

fn apply(app: &mut App, p: &P) -> Reply {
    use crate::play::{Apply, pair::Request};
    let s = app
        .pairing
        .published
        .as_ref()
        .ok_or_else(|| err(code::REFUSED, "pairing needs the editor window"))?;
    if s["state"] != "connected" {
        return Err(err(code::REFUSED, "not paired"));
    }
    let what = p.str("what")?;
    let number = |key| {
        p.opt_u64(key)?
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| {
                err(
                    code::INVALID_PARAMS,
                    format!("{key} must be a 32-bit integer"),
                )
            })
    };
    if matches!(what, "respawn" | "reload")
        && let Some(why) = s["edits_blocked"].as_str()
    {
        return Err(err(code::REFUSED, why));
    }
    let action = match what {
        "send" => {
            if let Some(why) = s["edit_refusal"].as_str() {
                return Err(err(code::REFUSED, why));
            }
            Apply::Send
        }
        "confirm" => {
            let peers = number("peers")?;
            let scope = app
                .pairing
                .peer_confirmation
                .filter(|s| s.peers == peers)
                .ok_or_else(|| {
                    err(code::REFUSED, "the host has no matching peer count to confirm")
                })?;
            Apply::Confirm(scope)
        }
        "respawn" => Apply::Respawn,
        "reload" => Apply::Reload,
        "adopt" => Apply::Adopt,
        "revert" => Apply::Revert,
        "keep" => Apply::Keep,
        "restore" => Apply::Restore,
        "take" => Apply::Take(number("at")?),
        "overwrite" => Apply::Overwrite(number("at")?),
        _ => return Err(err(code::INVALID_PARAMS, "unknown mirror.apply action")),
    };
    app.pairing.requests.push(Request::Apply(action));
    Ok(json!({"queued":what}))
}

fn status(app: &App) -> Value {
    let bridge = &app.pairing;
    let paired = bridge.published.as_ref().unwrap_or(&Value::Null);
    let meta = bridge
        .mirror
        .as_ref()
        .map(|m| m.status(bridge.latest || paired["paused"] == true))
        .unwrap_or_else(|| json!({"state":"off", "layers":[]}));
    let counters = bridge.counters.as_ref().unwrap_or(&Value::Null);
    let why = paired["edits_blocked"].as_str();
    let edit = paired["state"] == "connected" && paired["live_edits"] == true && why.is_none();
    json!({
        "stream": {
            "state": meta["state"], "protocol": meta["protocol"], "stream": meta["stream"],
            "tick_rate": meta["tick_rate"], "layers": meta["layers"], "map": bridge.stream_map, "refused":meta["refused"],
            "fps": meta["fps"], "delay_ms": meta["delay_ms"], "notice": meta["notice"],
            "dropped": counters["dropped"], "skipped": counters["skipped"]
        },
        "store": {
            "tick": meta["tick"], "first_tick": meta["first_tick"],
            "shown_tick": bridge.shown.as_ref().map(|s| s.tick),
            "objects": meta["objects"].as_u64().unwrap_or(0),
            "bytes": meta["bytes"].as_u64().unwrap_or(0)
        },
        "mode": if edit { "edit" } else { "view" },
        "why": if edit { None } else { why.or(Some("not paired")) }
    })
}

fn objects(app: &App, p: &P) -> Reply {
    let kind = p.opt_str("kind")?;
    let limit = p.opt_usize("limit")?.unwrap_or(usize::MAX);
    let near = if let Some(sphere) = p.get("near") {
        let sphere = P(sphere);
        let point = sphere.vec3("point")?;
        let radius = sphere
            .opt_f32("radius")?
            .filter(|v| v.is_finite() && *v >= 0.0)
            .ok_or_else(|| {
                err(
                    code::INVALID_PARAMS,
                    "near.radius must be a nonnegative finite number",
                )
            })?;
        Some((point, radius))
    } else {
        None
    };
    let exact;
    let shown = if let Some(tick) = p.opt_u64("at_tick")? {
        exact = app
            .pairing
            .mirror
            .as_ref()
            .and_then(|m| m.at_tick(tick))
            .ok_or_else(|| {
                err(
                    code::REFUSED,
                    format!("no sample at game tick {tick} in the current interpolation window"),
                )
            })?;
        Some(&exact)
    } else {
        app.pairing.shown.as_ref()
    };
    let Some(sample) = shown else {
        return Ok(json!({"tick":null, "objects":[], "truncated":false}));
    };
    let poses = near
        .as_ref()
        .map(|_| crate::viewport::mirror::WorldPoses::new(sample));
    let mut objects = Vec::new();
    let mut truncated = false;
    for o in &sample.objects {
        let group = ObjectType::from_u8(o.kind).map(ObjectType::group);
        if kind.is_some() && kind != group {
            continue;
        }
        if let Some(((point, radius), poses)) = near.zip(poses.as_ref()) {
            let Some(pos) = poses.position(o.id) else {
                continue;
            };
            let distance_squared: f32 = pos.iter().zip(point).map(|(a, b)| (a - b).powi(2)).sum();
            if distance_squared > radius * radius {
                continue;
            }
        }
        if objects.len() == limit {
            truncated = true;
            break;
        }
        objects.push(json!({
            "id":handle(o.id), "kind":group, "tag":o.tag.map(handle),
            "pos":o.position, "forward":o.forward, "up":o.up, "scale":o.scale,
            "velocity":o.velocity, "parent":o.parent.map(handle), "flags":o.flags,
            "body":o.body, "shield":o.shield, "placement":o.placement, "nodes":0
        }));
    }
    Ok(json!({"tick":sample.tick, "objects":objects, "truncated":truncated}))
}

fn pick(app: &App, p: &P) -> Reply {
    let hit = match (p.opt_str("from")?, p.get("ray")) {
        (Some("aim"), None) => select::from_aim(app),
        (None, Some(value)) => {
            let ray = P(value);
            let origin = ray.vec3("origin")?;
            let dir = ray.vec3("dir")?;
            let length_sq = scenario_edit::geometry::dot(dir, dir);
            if !origin.iter().all(|v| v.is_finite()) || !length_sq.is_finite() || length_sq <= 0.0 {
                return Err(err(
                    code::INVALID_PARAMS,
                    "ray needs a finite origin and nonzero finite direction",
                ));
            }
            select::pick(app, &Ray { origin, dir })
        }
        _ => {
            return Err(err(
                code::INVALID_PARAMS,
                "use either from: aim or a ray with origin and dir",
            ));
        }
    };
    Ok(hit.map_or(Value::Null, |hit| hit.json()))
}

fn aim_select(app: &mut App, p: &P) -> Reply {
    if p.has("enabled") {
        Play::set_aim_select(app, p.bool("enabled")?).map_err(|why| err(code::REFUSED, why))?;
    }
    let state = &app.pairing.selection;
    Ok(json!({"enabled":state.enabled,"candidate":state.candidate.as_ref().map(|hit| hit.json())}))
}

fn camera(app: &mut App, p: &P) -> Reply {
    let mode = match p.opt_str("mode")? {
        Some("free") => CameraMode::Free,
        Some("follow") => CameraMode::Follow,
        Some("eyes") => CameraMode::Eyes,
        None => app.pairing.camera.mode,
        Some(_) => {
            return Err(err(
                code::INVALID_PARAMS,
                "mode must be free, follow or eyes",
            ));
        }
    };
    if let Some(target) = p.opt_str("target")? {
        let index: u16 = target
            .strip_prefix("player.")
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| err(code::INVALID_PARAMS, "target must be player.<index>"))?;
        if mode == CameraMode::Free || Play::camera_target(app) != Some(index) {
            return Err(err(
                code::REFUSED,
                "the shared camera follows the local player",
            ));
        }
    }
    if p.has("mode") {
        Play::set_camera_mode(app, mode).map_err(|why| err(code::REFUSED, why))?;
    }
    let target = match mode {
        CameraMode::Free => None,
        CameraMode::Follow | CameraMode::Eyes => Play::camera_target(app),
    };
    Ok(json!({"mode":mode.name(),"target":target.map(|index| format!("player.{index}"))}))
}

fn move_player(app: &mut App, p: &P) -> Reply {
    let facing = p.bool("facing")?;
    let position = match p.get("to").and_then(Value::as_str) {
        Some("cursor") => Play::cursor_destination(app)
            .ok_or_else(|| err(code::REFUSED, "no level surface under the viewport cursor"))?,
        Some("camera") => crate::play::ground(app, app.session.camera().target),
        None => p.vec3("to")?,
        Some(_) => {
            return Err(err(
                code::INVALID_PARAMS,
                "to must be cursor, camera or [x,y,z]",
            ));
        }
    };
    if !position.iter().all(|v| v.is_finite()) {
        return Err(err(code::INVALID_PARAMS, "player position must be finite"));
    }
    let request =
        PlayerMove::capture(app, position, facing).map_err(|why| err(code::REFUSED, why))?;
    app.pairing.requests.push(Request::MovePlayer(request));
    Ok(json!({"requested":true}))
}
