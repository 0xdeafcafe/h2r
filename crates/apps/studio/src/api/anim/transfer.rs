//! Retargeting between graph skeletons and importing glTF clips.

use std::path::Path;

use agent_proto::RpcError;
use anim_edit::{NodeMapping, gltf_clips, read_gltf};
use map::TagId;
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::{MethodDef, P, Reply, anim_tag, def, err, failed};
use crate::App;
use crate::app::anim::{RetargetOptions, Transfer};

const RESULT: &str = r#"{"doc":"integer|null (null in preview)","name":"string","nodes":"[{source,source_name,target,target_name}]","unmatched_source":"[integer]","unmatched_target":"[integer]","dropped_channels":"[{node,channel}]","notes":"[string]"}"#;

pub(super) const RETARGET: MethodDef = def(
    m::ANIM_RETARGET,
    "Transfer an open animation document to the skeleton of a jmad tag in the shown map. Nodes match by name with explicit overrides; preview reports the mapping without creating a document",
    r#"{"from":"integer (source document)","to":"string (target jmad tag)","map?":"[{source: integer,target: integer|null}] (target null drops the node)","name?":"string","from_model?":"string (source graph user by name or id)","to_model?":"string (target graph user by name or id)","preview?":"boolean (default false)"}"#,
    RESULT,
    r#"{"from":1,"to":"jmad:invented\\stickman","name":"idle copy"}"#,
);

pub(super) const IMPORT: MethodDef = def(
    m::ANIM_IMPORT,
    "Import a glTF animation clip onto the skeleton of a jmad tag in the shown map. Select the clip by index or unique name if there is more than one. Preview reports the mapping without creating a document",
    r#"{"file":"string (local .gltf or .glb path)","to?":"string (target jmad tag; defaults to the selected graph)","clip?":"string|integer (name or index)","map?":"[{source: integer,target: integer|null}] (target null drops the node)","name?":"string","to_model?":"string (target graph user by name or id)","preview?":"boolean (default false)"}"#,
    RESULT,
    r#"{"file":"$OUT/wave.gltf","to":"jmad:invented\\stickman"}"#,
);

fn index(value: &Value) -> Option<usize> {
    value.as_u64().and_then(|n| usize::try_from(n).ok())
}

fn mapping(p: &P) -> Result<Vec<NodeMapping>, RpcError> {
    let Some(map) = p.get("map") else {
        return Ok(Vec::new());
    };
    let rows = map.as_array().ok_or_else(|| {
        err(code::INVALID_PARAMS, "map: expected an array of {source, target} mappings")
    })?;
    rows.iter()
        .enumerate()
        .map(|(row, entry)| {
            let bad = || {
                err(
                    code::INVALID_PARAMS,
                    format!("map[{row}]: expected {{source: index, target: index or null}}"),
                )
            };
            let object = entry.as_object().filter(|o| o.len() == 2).ok_or_else(bad)?;
            let source = object.get("source").and_then(index).ok_or_else(bad)?;
            let target = match object.get("target").ok_or_else(bad)? {
                Value::Null => None,
                value => Some(index(value).ok_or_else(bad)?),
            };
            Ok(NodeMapping { source, target })
        })
        .collect()
}

fn options<'p>(
    p: &'p P,
    mapping: &'p [NodeMapping],
) -> Result<RetargetOptions<'p>, RpcError> {
    Ok(RetargetOptions {
        mapping,
        name: p.opt_str("name")?,
        from_model: p.opt_str("from_model")?,
        to_model: p.opt_str("to_model")?,
        preview: p.bool("preview")?,
    })
}

fn target(app: &mut App, tag: Option<&str>) -> Result<TagId, RpcError> {
    if let Some(tag) = tag {
        return anim_tag(app, &P(&json!({"tag": tag})));
    }
    let tags = app.shown_tags();
    tags.selected
        .filter(|&id| tags.tag(id).is_some_and(|tag| tag.group.0 == *b"jmad"))
        .ok_or_else(|| err(code::INVALID_PARAMS, "to: name or select an animation graph (jmad)"))
}

fn result(transfer: Transfer) -> Value {
    let nodes: Vec<Value> = transfer
        .report
        .nodes
        .iter()
        .map(|row| {
            let source_name = transfer.source[row.source].name.as_deref();
            let target_name = row.target.and_then(|i| transfer.target[i].name.as_deref());
            json!({
                "source": row.source,
                "source_name": source_name,
                "target": row.target,
                "target_name": target_name,
            })
        })
        .collect();
    let dropped_channels: Vec<Value> = transfer
        .report
        .dropped_channels
        .iter()
        .map(|channel| json!({"node": channel.node, "channel": channel.kind.name()}))
        .collect();
    json!({
        "doc": transfer.doc,
        "name": transfer.name,
        "nodes": nodes,
        "unmatched_source": transfer.report.unmatched_source,
        "unmatched_target": transfer.report.unmatched_target,
        "dropped_channels": dropped_channels,
        "notes": transfer.notes,
    })
}

pub(super) fn retarget(app: &mut App, p: &P) -> Reply {
    let from = p.u64("from")?;
    let to = target(app, Some(p.str("to")?))?;
    let mapping = mapping(p)?;
    let transfer = app.anim_retarget(from, to, options(p, &mapping)?).map_err(failed)?;
    Ok(result(transfer))
}

fn clip_index(clips: &[String], selector: Option<&Value>) -> Result<usize, RpcError> {
    let why = match selector {
        None if clips.len() == 1 => return Ok(0),
        None if clips.is_empty() => "file has no animation clips".to_owned(),
        None => "clip: select a glTF animation by index or unique name".to_owned(),
        Some(Value::String(name)) => {
            let mut matches = clips.iter().enumerate().filter(|(_, clip)| *clip == name);
            match matches.next() {
                Some((index, _)) if matches.next().is_none() => return Ok(index),
                Some(_) => format!("clip: the name {name:?} is ambiguous, use its index"),
                None => format!("clip: no animation named {name:?}"),
            }
        }
        Some(value) => {
            if let Some(i) = index(value).filter(|&i| i < clips.len()) {
                return Ok(i);
            }
            format!("clip: expected an animation name or a valid index, got {value}")
        }
    };
    let mut error = err(code::INVALID_PARAMS, why);
    let choices: Vec<Value> = clips
        .iter()
        .enumerate()
        .map(|(index, name)| json!({"index": index, "name": name}))
        .collect();
    error.data = Some(json!({"clips": choices}));
    Err(error)
}

pub(super) fn import(app: &mut App, p: &P) -> Reply {
    let path = Path::new(p.str("file")?);
    let mapping = mapping(p)?;
    let options = options(p, &mapping)?;
    let to = target(app, p.opt_str("to")?)?;
    let refused = |error| err(code::REFUSED, error);
    let clips = gltf_clips(path).map_err(refused)?;
    let index = clip_index(&clips, p.get("clip"))?;
    let imported = read_gltf(path, index).map_err(refused)?;
    let transfer = app.anim_import(&imported, to, options).map_err(failed)?;
    Ok(result(transfer))
}
