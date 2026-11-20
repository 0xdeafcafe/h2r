//! The viewport's current mirror and the exact game samples it interpolates.
use agent_proto::sim::objects::handle;
use guest_layout::ObjectType;
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::{MethodDef, P, Provider, Reply, def, err};
use crate::App;

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
            m::MIRROR_OBJECTS => Some(objects(app, &P(params))),
            _ => None,
        }
    }
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
