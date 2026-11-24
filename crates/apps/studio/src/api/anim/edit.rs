//! In-memory animation documents shared with the Animation window.

use anim_edit::{AnimDoc, Interp, Key, NodeCheck};
use serde_json::{Value, json};
use studio_proto::{code, method as m};
use tags::anim_keys::Kind;

use super::{HZ, MethodDef, P, Reply, anim_tag, def, err, failed, find_animation, kind_value};
use crate::App;

pub(super) const NEW: MethodDef = def(
    m::ANIM_NEW,
    "Create an empty animation document on the skeleton of a jmad tag in the shown map",
    r#"{"tag":"string","name":"string","frames":"positive integer"}"#,
    r#"{"doc":"integer","name":"string","frames":"integer","nodes":"integer","revision":"integer"}"#,
    r#"{"tag":"jmad:invented\\stickman","name":"wave","frames":30}"#,
);

pub(super) const DUPLICATE: MethodDef = def(
    m::ANIM_DUPLICATE,
    "Start a separate animation document from a decoded clip or a snapshot of an open document",
    r#"{"tag?":"string (with animation)","animation?":"string|integer (its name or index)","doc?":"integer (instead of tag and animation)","name":"string"}"#,
    r#"{"doc":"integer","name":"string","frames":"integer","nodes":"integer","revision":"integer"}"#,
    r#"{"tag":"jmad:invented\\stickman","animation":"combat:idle","name":"idle copy"}"#,
);

pub(super) const RETARGET: MethodDef = def(
    m::ANIM_RETARGET,
    "Carry an open base clip onto the skeleton of a jmad tag in the shown map as a new document. Nodes pair by name, the table pairs the rest; rotations carry as deltas from each rest pose, translations scale by bone length. Unmatched nodes are listed and their channels dropped",
    r#"{"doc":"integer","to":"string (the target jmad tag)","name":"string","table?":"[[source node index, target node index]]","from_model?":"string","to_model?":"string (the model whose rest pose the graph's skeleton takes, when several use it)"}"#,
    r#"{"doc":"integer","name":"string","frames":"integer","nodes":"integer","revision":"integer","matched":"[[source,target]]","dropped_nodes":"[integer]","dropped_channels":"integer","unmatched_targets":"[integer]","scaled":"[{node,ratio}]"}"#,
    r#"{"doc":1,"to":"jmad:invented\\stickman","name":"wave retargeted"}"#,
);

pub(super) const KEY: MethodDef = def(
    m::ANIM_KEY,
    "Set or remove one key on an open animation document. An interpolation edit without a value needs an existing key. Interpolation governs the segment after the key",
    r#"{"doc":"integer","node":"string|integer (unique name or index)","channel":"rotation|translation|scale","frame":"integer","value?":"[x,y,z,w] for rotation, [x,y,z] for translation, [s] for scale","interp?":"step|linear|smooth","remove?":"boolean (exclusive with value and interp; default false)"}"#,
    r#"{"doc":"integer","revision":"integer","changed":"boolean","key":"{frame,value:[number],interp,tangent:[number]}|null"}"#,
    r#"{"doc":1,"node":1,"channel":"translation","frame":2,"value":[1,0,0],"interp":"smooth"}"#,
);

pub(super) const CHECK: MethodDef = def(
    m::ANIM_CHECK,
    "Measure codec quantisation error per node, named feet below ground, and the pose difference across the loop seam",
    r#"{"doc":"integer","model?":"string (a model user by name or 0x... id; default its only one)"}"#,
    r#"{"doc":"integer","nodes":"[{node,rotation_degrees,translation,scale}]","feet_checked":"boolean","feet_below_ground":"[{node,frame,z}]","loop_seam":"[{node,rotation_degrees,translation,scale}]"}"#,
    r#"{"doc":1}"#,
);

pub(super) fn doc_only(p: &P, source_params: &[&str]) -> Result<(), agent_proto::RpcError> {
    if let Some(param) = source_params.iter().find(|param| p.has(param)) {
        return Err(invalid(format!("doc and {param} cannot be combined")));
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> agent_proto::RpcError {
    err(code::INVALID_PARAMS, message)
}

fn doc_summary(app: &App, id: u64) -> Reply {
    let doc = app.anim_doc(id).map_err(failed)?;
    Ok(json!({
        "doc": id,
        "name": doc.name(),
        "frames": doc.frames(),
        "nodes": doc.nodes().len(),
        "revision": doc.revision(),
    }))
}

pub(super) fn new(app: &mut App, p: &P) -> Reply {
    let tag = anim_tag(app, p)?;
    let id = app
        .anim_new(tag, p.str("name")?, p.usize("frames")?)
        .map_err(failed)?;
    doc_summary(app, id)
}

pub(super) fn duplicate(app: &mut App, p: &P) -> Reply {
    let name = p.str("name")?;
    let id = if let Some(doc) = p.opt_u64("doc")? {
        doc_only(p, &["tag", "animation"])?;
        app.anim_duplicate_doc(doc, name).map_err(failed)?
    } else {
        let tag = anim_tag(app, p)?;
        let info = app.anim_info(tag).map_err(failed)?;
        let animation = find_animation(&info, p.get("animation").unwrap_or(&Value::Null))?;
        app.anim_duplicate(tag, animation, name).map_err(failed)?
    };
    doc_summary(app, id)
}

pub(super) fn retarget(app: &mut App, p: &P) -> Reply {
    let doc = p.u64("doc")?;
    let name = p.str("name")?;
    let to = super::anim_tag(app, &P(&json!({"tag": p.get("to").cloned().unwrap_or(Value::Null)})))?;
    let mut table = Vec::new();
    if let Some(rows) = p.get("table") {
        let bad = || invalid("table: expected [[source node index, target node index]]");
        for row in rows.as_array().ok_or_else(bad)? {
            let pair = row.as_array().filter(|a| a.len() == 2).ok_or_else(bad)?;
            let index = |v: &Value| v.as_u64().and_then(|i| usize::try_from(i).ok()).ok_or_else(bad);
            table.push((index(&pair[0])?, index(&pair[1])?));
        }
    }
    let (id, report) = app
        .anim_retarget(doc, to, name, &table, [p.opt_str("from_model")?, p.opt_str("to_model")?])
        .map_err(failed)?;
    let mut out = doc_summary(app, id)?;
    out["matched"] = json!(report.matched);
    out["dropped_nodes"] = json!(report.dropped_nodes);
    out["dropped_channels"] = json!(report.dropped_channels);
    out["unmatched_targets"] = json!(report.unmatched_targets);
    out["scaled"] = report
        .scaled
        .iter()
        .map(|(node, ratio)| json!({"node": node, "ratio": ratio}))
        .collect();
    Ok(out)
}

pub(super) fn read(app: &App, id: u64, with_keys: bool) -> Reply {
    let doc = app.anim_doc(id).map_err(failed)?;
    let mut rows = vec![Vec::new(); doc.nodes().len()];
    for (node, kind, keys) in doc.channels() {
        let mut channel = json!({
            "kind": kind.name(),
            "moves": keys.len() > 1,
            "key_count": keys.len(),
        });
        if with_keys {
            channel["keys"] = keys.iter().map(|key| key_json(kind, key)).collect();
        }
        rows[node].push(channel);
    }
    let nodes: Vec<Value> = doc
        .nodes()
        .iter()
        .zip(rows)
        .enumerate()
        .map(|(index, (node, channels))| {
            json!({"index": index, "name": node.name, "parent": node.parent, "channels": channels})
        })
        .collect();
    Ok(json!({
        "doc": id,
        "name": doc.name(),
        "frame_rate": HZ,
        "frames": doc.frames(),
        "duration": doc.frames() as f32 / HZ,
        "revision": doc.revision(),
        "nodes": nodes,
    }))
}

fn key_json(kind: Kind, key: &Key) -> Value {
    json!({
        "frame": key.frame,
        "value": kind_value(kind, key.value),
        "interp": key.interp.name(),
        "tangent": kind_value(kind, key.tangent),
    })
}

fn node_index(doc: &AnimDoc, want: &Value) -> Result<usize, agent_proto::RpcError> {
    if let Some(name) = want.as_str() {
        let mut matches = doc
            .nodes()
            .iter()
            .enumerate()
            .filter(|(_, node)| node.name.as_deref() == Some(name))
            .map(|(i, _)| i);
        let index = matches
            .next()
            .ok_or_else(|| invalid(format!("no node named {name:?}")))?;
        if matches.next().is_none() {
            return Ok(index);
        }
        return Err(invalid(format!(
            "node name {name:?} is ambiguous, use its index"
        )));
    }
    want.as_u64()
        .and_then(|i| usize::try_from(i).ok())
        .filter(|&i| i < doc.nodes().len())
        .ok_or_else(|| {
            invalid(format!(
                "node: expected a name or index of the document's {} nodes",
                doc.nodes().len()
            ))
        })
}

fn key_value(kind: Kind, value: &Value) -> Result<[f32; 4], agent_proto::RpcError> {
    let bad = || {
        invalid(format!(
            "value: expected {} finite numbers for {}",
            kind.width(),
            kind.name()
        ))
    };
    let values = value
        .as_array()
        .filter(|a| a.len() == kind.width())
        .ok_or_else(bad)?;
    let mut out = [0.0; 4];
    for (dest, value) in out.iter_mut().zip(values) {
        *dest = value
            .as_f64()
            .map(|v| v as f32)
            .filter(|v| v.is_finite())
            .ok_or_else(bad)?;
    }
    Ok(out)
}

pub(super) fn key(app: &mut App, p: &P) -> Reply {
    let id = p.u64("doc")?;
    let kind = match p.str("channel")? {
        "rotation" => Kind::Rotation,
        "translation" => Kind::Translation,
        "scale" => Kind::Scale,
        _ => return Err(invalid("channel: expected rotation, translation or scale")),
    };
    let frame = p.usize("frame")?;
    let doc = app.anim_doc_mut(id).map_err(failed)?;
    let node = node_index(doc, p.get("node").unwrap_or(&Value::Null))?;
    let value = p.get("value");
    let interp = p.opt_str("interp")?;
    let changed = if p.bool("remove")? {
        if value.is_some() || interp.is_some() {
            return Err(invalid("remove cannot be combined with value or interp"));
        }
        doc.remove_key(node, kind, frame).map_err(invalid)?
    } else {
        let interp = match interp {
            None => None,
            Some("step") => Some(Interp::Step),
            Some("linear") => Some(Interp::Linear),
            Some("smooth") => Some(Interp::Smooth),
            Some(_) => return Err(invalid("interp: expected step, linear or smooth")),
        };
        let value = match (value, interp) {
            (Some(value), _) => key_value(kind, value)?,
            (None, Some(_)) => {
                doc.key(node, kind, frame)
                    .ok_or_else(|| invalid("a new key requires a value"))?
                    .value
            }
            (None, None) => return Err(invalid("value, interp or remove:true is required")),
        };
        doc.set_key(node, kind, frame, value, interp)
            .map_err(invalid)?
    };
    let key = doc.key(node, kind, frame).map(|key| key_json(kind, &key));
    Ok(json!({"doc": id, "revision": doc.revision(), "changed": changed, "key": key}))
}

fn check_node(node: &NodeCheck) -> Value {
    json!({
        "node": node.node,
        "rotation_degrees": node.rotation.to_degrees(),
        "translation": node.translation,
        "scale": node.scale,
    })
}

pub(super) fn check(app: &mut App, p: &P) -> Reply {
    let id = p.u64("doc")?;
    let check = app.anim_check(id, p.opt_str("model")?).map_err(failed)?;
    let contacts: Vec<Value> = check
        .feet_below_ground
        .iter()
        .map(|contact| json!({"node": contact.node, "frame": contact.frame, "z": contact.z}))
        .collect();
    Ok(json!({
        "doc": id,
        "nodes": check.nodes.iter().map(check_node).collect::<Vec<_>>(),
        "feet_checked": check.feet_checked,
        "feet_below_ground": contacts,
        "loop_seam": check.loop_seam.iter().map(check_node).collect::<Vec<_>>(),
    }))
}
