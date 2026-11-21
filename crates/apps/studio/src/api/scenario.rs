//! `scenario.*`: the placement blocks of the open map, read and edited
//! through the same actions as the outliner, inspector and gizmo.

use agent_proto::RpcError;
use map::{FourCC, TagId};
use scenario_edit::layout::{self, FieldType, Role};
use scenario_edit::{BlockKind, Scenario, TagRef, Value as FieldValue};
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::params::{block_name, vec3};
use super::{P, Reply, err};
use crate::App;
use crate::app::{Action, Refused};

/// The open map's scenario, or NO_DOCUMENT.
pub fn open(app: &App) -> Result<&Scenario, RpcError> {
    app.session.scenario().ok_or_else(|| {
        err(
            code::NO_DOCUMENT,
            "no map is open: source.open a map, or doc.open {\"map\": ...}",
        )
    })
}

/// An [`App::perform`] refusal as an API error.
pub fn refused(r: Refused) -> RpcError {
    match r {
        Refused::Unsaved(_) => err(code::UNSAVED, r.to_string()),
        Refused::Failed(e) if e.contains("refused") || e.contains("read-only") => {
            err(code::REFUSED, e)
        }
        Refused::Failed(e) if e.contains("I/O") || e.contains("os error") => err(code::FAILED, e),
        Refused::Failed(e) => err(code::INVALID_PARAMS, e),
    }
}

pub fn tag_id(id: TagId) -> String {
    format!("{:#010x}", id.0)
}

pub fn group_text(g: FourCC) -> String {
    g.to_string().trim_end().to_owned()
}

/// A tag reference as JSON: `null` for none.
pub fn tag_ref_json(s: &Scenario, r: TagRef) -> Value {
    if r.is_none() {
        return Value::Null;
    }
    json!({
        "id": tag_id(r.id),
        "group": group_text(r.group),
        "name": s.tags.get(&r.id).and_then(|t| t.name.clone()),
    })
}

pub fn value_json(s: &Scenario, v: FieldValue) -> Value {
    match v {
        FieldValue::Int(i) => json!(i),
        FieldValue::Float(f) => json!(f),
        FieldValue::Vec3(v) => json!(v),
        FieldValue::Tag(r) => tag_ref_json(s, r),
    }
}

/// A tag id from `0x...` text or a number.
pub fn parse_tag_id(v: &Value) -> Option<TagId> {
    match v {
        Value::Number(n) => n.as_u64().and_then(|n| u32::try_from(n).ok()).map(TagId),
        Value::String(s) => {
            let h = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"))?;
            u32::from_str_radix(h, 16).ok().map(TagId)
        }
        _ => None,
    }
}

/// JSON as the field's value type. The scenario checks ranges, palettes
/// and tag groups when the edit is applied.
pub fn field_value(
    s: &Scenario,
    kind: BlockKind,
    field: &str,
    v: &Value,
) -> Result<FieldValue, RpcError> {
    let def = layout::block(kind)
        .fields
        .iter()
        .find(|f| f.name == field)
        .ok_or_else(|| {
            err(
                code::INVALID_PARAMS,
                crate::app::commands::field_name(kind, field).unwrap_err(),
            )
        })?;
    let bad = |want: &str| {
        err(
            code::INVALID_PARAMS,
            format!("{field} is {:?}: expected {want}, not {v}", def.ty),
        )
    };
    Ok(match def.ty {
        FieldType::U8 | FieldType::I16 | FieldType::U16 | FieldType::U32 => {
            FieldValue::Int(v.as_i64().ok_or_else(|| bad("a whole number"))?)
        }
        FieldType::F32 => FieldValue::Float(
            v.as_f64()
                .filter(|f| f.is_finite())
                .ok_or_else(|| bad("a finite number"))? as f32,
        ),
        FieldType::Vec3 => FieldValue::Vec3(vec3(field, v)?),
        FieldType::TagRef => {
            if v.is_null() {
                FieldValue::Tag(TagRef::NONE)
            } else {
                let id = parse_tag_id(v.get("id").unwrap_or(v))
                    .ok_or_else(|| bad("{\"id\": \"0x...\"}, an id, or null"))?;
                let info = s.tags.get(&id).ok_or_else(|| {
                    err(
                        code::REFUSED,
                        format!("{field}: tag {} is not in this map", tag_id(id)),
                    )
                })?;
                FieldValue::Tag(TagRef {
                    group: info.group,
                    id,
                })
            }
        }
    })
}

fn role_name(r: Role) -> &'static str {
    match r {
        Role::Position => "position",
        Role::Rotation => "rotation",
        Role::Facing => "facing",
        Role::PaletteIndex => "palette_index",
        Role::Scale => "scale",
        Role::Plain => "plain",
    }
}

fn entry_json(s: &Scenario, kind: BlockKind, index: usize) -> Value {
    let mut v = json!({
        "block": block_name(kind),
        "index": index,
        "label": s.label(kind, index),
    });
    if let Some(f) = layout::block(kind).role(Role::Position)
        && let Ok(p) = s.get(kind, index, f.name)
    {
        v["position"] = value_json(s, p);
    }
    v
}

pub fn get_json(s: &Scenario, kind: BlockKind, index: usize) -> Value {
    let mut fields = serde_json::Map::new();
    for f in layout::block(kind).fields {
        if let Ok(v) = s.get(kind, index, f.name) {
            fields.insert(f.name.to_owned(), value_json(s, v));
        }
    }
    let mut v = entry_json(s, kind, index);
    v["tag"] = s
        .object_tag(kind, index)
        .map(|r| tag_ref_json(s, r))
        .unwrap_or(Value::Null);
    v["named"] = json!(s.names_of(kind, index).len());
    v["fields"] = Value::Object(fields);
    v
}

/// Runs a scenario action for the API.
fn perform(app: &mut App, a: Action) -> Result<Vec<String>, RpcError> {
    app.perform(a, false).map_err(refused)
}

pub fn call(app: &mut App, method: &str, p: &P) -> Reply {
    match method {
        m::SCENARIO_BLOCKS => {
            let s = open(app)?;
            Ok(Value::Array(
                layout::BLOCKS
                    .iter()
                    .map(|b| {
                        let block = s.block(b.kind);
                        json!({
                            "block": block_name(b.kind),
                            "label": b.label,
                            "count": block.len(),
                            "capacity": block.capacity(),
                            "stride": b.stride,
                            "palette": block.palette.len(),
                            "max_count": b.max_count,
                            "fields": b.fields.iter().map(|f| json!({
                                "name": f.name,
                                "type": format!("{:?}", f.ty).to_ascii_lowercase(),
                                "role": role_name(f.role),
                                "editable": f.editable,
                                "offset": f.offset,
                            })).collect::<Vec<_>>(),
                            "add_refusal": app.session.add_refusal(b.kind),
                            "remove_refusal": app.session.remove_refusal(b.kind),
                        })
                    })
                    .collect(),
            ))
        }
        m::SCENARIO_LIST => {
            let s = open(app)?;
            let kinds: Vec<BlockKind> = match p.has("block") {
                true => vec![p.block()?],
                false => layout::BLOCKS.iter().map(|b| b.kind).collect(),
            };
            Ok(Value::Array(
                kinds
                    .into_iter()
                    .flat_map(|k| (0..s.block(k).len()).map(move |i| (k, i)))
                    .map(|(k, i)| entry_json(s, k, i))
                    .collect(),
            ))
        }
        m::SCENARIO_GET => {
            let (kind, index) = p.entry(app)?;
            Ok(get_json(open(app)?, kind, index))
        }
        m::SCENARIO_SET => {
            let (block, index) = p.entry(app)?;
            let field = p.str("field")?;
            let value = field_value(
                open(app)?,
                block,
                field,
                p.get("value").unwrap_or(&Value::Null),
            )?;
            perform(
                app,
                Action::SetField {
                    block,
                    index,
                    field: field.to_owned(),
                    value,
                    merge: None,
                },
            )?;
            let s = open(app)?;
            Ok(
                json!({ "value": value_json(s, s.get(block, index, field).map_err(|e| err(code::FAILED, e.to_string()))?) }),
            )
        }
        m::SCENARIO_ADD => {
            let kind = p.block()?;
            add(app, kind, p.opt_usize("like")?, p.opt_vec3("position")?)
        }
        m::SCENARIO_DUPLICATE => {
            let (kind, index) = p.entry(app)?;
            add(app, kind, Some(index), None)
        }
        m::SCENARIO_MOVE => {
            let (block, index) = p.entry(app)?;
            let position = p.vec3("position")?;
            perform(
                app,
                Action::MovePlacement {
                    block,
                    index,
                    position,
                    ypr: p.opt_vec3("ypr")?,
                },
            )?;
            Ok(json!({ "position": position }))
        }
        m::SCENARIO_DELETE => {
            let (kind, index) = p.entry(app)?;
            if let Some(why) = app.session.remove_refusal(kind) {
                return Err(err(code::REFUSED, why));
            }
            perform(app, Action::DeleteEntry(kind, index))?;
            Ok(json!({ "removed": true }))
        }
        m::SCENARIO_SELECT => {
            if p.has("tag") {
                if p.has("block") || p.has("index") || p.has("focus") {
                    return Err(err(code::INVALID_PARAMS, "tag cannot be combined with block, index or focus"));
                }
                let tag = super::nav::open_tag(app, p)?;
                perform(app, Action::Navigate(crate::app::NavAction::SelectTag(tag)))?;
                return Ok(json!({"selected": super::nav::selection_json(app.selection())}));
            }
            let sel = if p.has("block") {
                Some(p.entry(app)?)
            } else {
                None
            };
            perform(app, Action::Select(sel))?;
            if sel.is_some() && p.bool("focus")? {
                perform(app, Action::FocusSelected)?;
            }
            Ok(json!({
                "selected": app.session.selected().map(|(k, i)| json!({"block": block_name(k), "index": i})),
            }))
        }
        _ => Err(err(code::METHOD_NOT_FOUND, format!("no method {method}"))),
    }
}

/// Adds a placement: refused first where the block can't grow safely or
/// takes no new entries. The editor puts a new entry (or a copy) at the
/// camera focus; at `position`, the focus is moved there for the add and
/// back after, so the add stays one undo step.
pub fn add(
    app: &mut App,
    kind: BlockKind,
    like: Option<usize>,
    position: Option<[f32; 3]>,
) -> Reply {
    let s = open(app)?;
    if let Some(i) = like
        && i >= s.block(kind).len()
    {
        return Err(err(
            code::INVALID_PARAMS,
            format!("{} has no entry {i} to copy", block_name(kind)),
        ));
    }
    if let Some(why) = app.session.add_refusal(kind) {
        let mut e = err(code::REFUSED, format!("{}: {why}", block_name(kind)));
        e.data = Some(json!({
            "count": s.block(kind).len(),
            "capacity": s.block(kind).capacity(),
        }));
        return Err(e);
    }
    let index = s.block(kind).len();
    let keep = app.session.camera().target;
    if let Some(p) = position {
        app.session.camera_mut().target = p;
    }
    let action = match like {
        Some(i) => Action::DuplicateEntry(kind, i),
        None => Action::Add(kind),
    };
    let r = perform(app, action);
    app.session.camera_mut().target = keep;
    r?;
    Ok(json!({ "index": index }))
}
