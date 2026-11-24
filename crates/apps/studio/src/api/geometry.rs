//! The Modeling tool as the agent API's `geometry.*` and `mesh.*`
//! ([`super::Provider`], registered in [`super::seams`]): the open map's
//! targets, edit mode on one, its mesh, mesh operations as one undo step
//! each and the write check. Every edit goes through `Action::Model`, so
//! an agent's edit is the window's undo step and saves with the map;
//! docs/editor/modeling.md, docs/editor/agent-api.md.

use agent_proto::RpcError;
use geometry_edit::Target;
use geometry_edit::mesh_edit::{MeshOp, SelectHow, SelectMode};
use map::TagId;
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::scenario::refused;
use super::{MethodDef, P, Provider, Reply, err};
use crate::App;
use crate::app::modeling::ModelAction;
use crate::app::{Action, Refused};

/// The Modeling tool's provider.
#[derive(Default)]
pub struct Geometry;

const fn def(
    name: &'static str,
    summary: &'static str,
    params: &'static str,
    result: &'static str,
    example: &'static str,
) -> MethodDef {
    MethodDef {
        name,
        summary,
        params,
        result,
        example,
        planned: None,
    }
}

/// `target` is `kind:tag:index` (`cluster:e1000004:0`, `instance_def:…`,
/// `mode_section:…`, `coll_bsp:…`); the target in edit mode otherwise.
const METHODS: &[MethodDef] = &[
    def(
        m::GEOMETRY_LIST,
        "The open map's editable geometry: every structure BSP's clusters and instanced definitions, and a render model's sections when `model` names one (its tag id), with counts, whether the raw data is the map's own and whether it is open",
        r#"{"model?":"string"}"#,
        r#"[{"target":"string","kind":"cluster|instance_def|mode_section|coll_bsp","tag":"string","name":"string","vertices":"integer","triangles":"integer","parts":"integer","local":"boolean","open":"boolean"}]"#,
        r#"{}"#,
    ),
    def(
        m::GEOMETRY_OPEN,
        "Enter edit mode on a target: its section is read as a mesh (coincident vertices welded) and the map's undo becomes its undo",
        r#"{"target":"string"}"#,
        r#"{"target":"string","verts":"integer","edges":"integer","faces":"integer","welded":"integer","passthrough":"integer"}"#,
        r#"{"target":"cluster:e1000004:0"}"#,
    ),
    def(
        m::GEOMETRY_CLOSE,
        "Leave edit mode on a target; its edits stay and save with the map",
        r#"{"target?":"string"}"#,
        r#"{"target":"string","edited":"boolean"}"#,
        r#"{"target":"cluster:e1000004:0"}"#,
    ),
    def(
        m::GEOMETRY_READ,
        "An open target's mesh: positions by vertex id, faces as vertex ids with material and group, the select mode and the selected vertices",
        r#"{"target?":"string"}"#,
        r#"{"target":"string","mode":"vert|edge|face","verts":"[{id,position}]","faces":"[{id,verts,material,group}]","selected":"[integer]"}"#,
        r#"{"target":"cluster:e1000004:0"}"#,
    ),
    def(
        m::GEOMETRY_CHECK,
        "Each open target's write class (unchanged, in_place, grown_raw), counts, moved vertices, lightmap-stale faces and why it cannot be written; `save_refusal` is why a save would be refused now (an edit the format cannot hold, or Allow unverified geometry writes off)",
        r#"{"target?":"string"}"#,
        r#"{"targets":"[{target,class,verts,faces,moved,lightmap_stale,ops,refused}]","allow_unverified":"boolean","save_refusal":"string|null"}"#,
        r#"{}"#,
    ),
    def(
        m::MODEL_COLLISION_FROM_RENDER,
        "Replace an open collision BSP with polygons from an open render section in the same coordinate space. Optional materials maps each render material index to a collision material (default all 0). One undo step",
        r#"{"render":"string","target?":"string","materials?":"[integer]"}"#,
        r#"{"target":"string","verts":"integer","faces":"integer"}"#,
        r#"{"render":"cluster:e1000004:0","target":"coll_bsp:e100000a:0"}"#,
    ),
    def(
        m::GEOMETRY_LOWER_LODS,
        "Use for lower LODs: point every lower level of detail of each permutation that draws an open model section (`mode_section:…`) at it (`on`, default true), or leave them as the file has them (false): one undo step, saved as patches of the permutations' LOD indices. A target that is not a model section is refused",
        r#"{"target?":"string","on?":"boolean"}"#,
        r#"{"target":"string","on":"boolean","changed":"boolean"}"#,
        r#"{"target":"mode_section:e1000002:0"}"#,
    ),
    def(
        m::GEOMETRY_COLLISION_ADD_BOX,
        "Add a closed box solid inside [lo, hi] to a structure BSP's level collision (`structure`, its tag id), onto the boxes added before: one Modeling action. A paired game gets it live (edit.collision.stage then edit.collision.commit, append); a save writes it (grown blocks, with Allow unverified geometry writes on). `material` indexes the structure's collision materials (default 0)",
        r#"{"structure":"string","lo":"[x,y,z]","hi":"[x,y,z]","material?":"integer"}"#,
        r#"{"structure":"string","surfaces":"integer","planes":"integer","leaves":"integer","file_surfaces":"integer"}"#,
        r#"{"structure":"e1000004","lo":[2,2,1],"hi":[3,3,2]}"#,
    ),
    def(
        m::GEOMETRY_COLLISION_CLEAR,
        "Drop every box added to a structure's level collision: the file's collision again, in the save and in a paired game (which stages the file's blocks again)",
        r#"{"structure":"string"}"#,
        r#"{"structure":"string","cleared":"boolean"}"#,
        r#"{"structure":"e1000004"}"#,
    ),
    def(
        m::MESH_MODE,
        "Set the select mode of the target in edit mode (one undo step, as every mesh.* edit)",
        r#"{"mode":"vert|edge|face","target?":"string"}"#,
        r#"{"target":"string","mode":"string"}"#,
        r#"{"mode":"face"}"#,
    ),
    def(
        m::MESH_SELECT,
        "Select elements of the mode's kind: `ids` with `op` (set, add, remove, toggle; default set), or `all`, `none` or `invert`",
        r#"{"ids?":"[integer]","op?":"set|add|remove|toggle","all?":"boolean","none?":"boolean","invert?":"boolean","target?":"string"}"#,
        r#"{"target":"string","selected":"integer"}"#,
        r#"{"all":true}"#,
    ),
    def(
        m::MESH_TRANSFORM,
        "Move the selection by `translate`, or by a 3 by 4 affine `matrix` ([L|t] rows) about `pivot` (default the selection's centre); `merge` (a key) joins a drag's steps into one until a call without it",
        r#"{"translate?":"[x,y,z]","matrix?":"[[number;4];3]","pivot?":"[x,y,z]","merge?":"integer","target?":"string"}"#,
        r#"{"target":"string","class":"string","moved":"integer"}"#,
        r#"{"translate":[0,0,0.5]}"#,
    ),
    def(
        m::MESH_OPS,
        "The op log of a target: every MeshOp applied since it was opened, in order; mesh.apply replays it",
        r#"{"target?":"string"}"#,
        r#"{"target":"string","ops":"[MeshOp]"}"#,
        r#"{}"#,
    ),
    def(
        m::MESH_APPLY,
        "Apply a list of MeshOps (as mesh.ops gives them) as one undo step: all of them or, when one fails, none",
        r#"{"ops":"[MeshOp]","target?":"string"}"#,
        r#"{"target":"string","class":"string","ops":"integer"}"#,
        r#"{"ops":[{"op":"select_all"},{"op":"select_none"}]}"#,
    ),
];

impl Provider for Geometry {
    fn methods(&self) -> Vec<MethodDef> {
        METHODS.to_vec()
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        if !METHODS.iter().any(|d| d.name == method) {
            return None;
        }
        Some(call(app, method, &P(params)))
    }
}

fn run(app: &mut App, a: ModelAction) -> Result<Vec<String>, RpcError> {
    app.perform(Action::Model(a), false).map_err(refused)
}

/// The `target` param, else the target in edit mode.
fn target(app: &App, p: &P) -> Result<Target, RpcError> {
    match p.opt_str("target")? {
        Some(id) => parse_target("target", id),
        None => app.modeling.session.active.ok_or_else(|| {
            err(
                code::NO_DOCUMENT,
                "no target is in edit mode (geometry.open)",
            )
        }),
    }
}

fn parse_target(param: &str, id: &str) -> Result<Target, RpcError> {
    Target::parse(id).ok_or_else(|| {
        err(
            code::INVALID_PARAMS,
            format!("{param}: {id:?} is not kind:tag:index (cluster:e1000004:0)"),
        )
    })
}

/// [`target`], which must be open.
fn open_target(app: &App, p: &P) -> Result<Target, RpcError> {
    let t = target(app, p)?;
    if !app.modeling.session.is_open(t) {
        return Err(err(
            code::NO_DOCUMENT,
            format!("{t} is not open (geometry.open)"),
        ));
    }
    Ok(t)
}

fn apply(app: &mut App, t: Target, ops: Vec<MeshOp>, merge: Option<u64>) -> Result<(), RpcError> {
    if merge.is_none() {
        run(app, ModelAction::Seal)?;
    }
    run(
        app,
        ModelAction::Apply {
            target: t,
            ops,
            merge,
        },
    )?;
    Ok(())
}

/// The target's write class and moved vertices.
fn summary(app: &App, t: Target) -> Value {
    let c = app.modeling.session.check(Some(t));
    let (class, moved) = c
        .first()
        .map_or(("unchanged", 0), |c| (c.class.name(), c.moved));
    json!({ "target": t.id(), "class": class, "moved": moved })
}

fn call(app: &mut App, method: &str, p: &P) -> Reply {
    app.sync_modeling();
    match method {
        m::GEOMETRY_LIST => {
            let model = match p.opt_str("model")? {
                Some(s) => Some(parse_tag(s)?),
                None => None,
            };
            let list = app
                .modeling_targets(model)
                .map_err(|e| err(code::FAILED, e))?;
            let open = &app.modeling.session;
            Ok(Value::Array(
                list.iter()
                    .map(|t| {
                        json!({
                            "target": t.target.id(),
                            "kind": t.target.kind.name(),
                            "tag": format!("{:#010x}", t.target.tag.0),
                            "name": t.tag_name,
                            "vertices": t.vertices,
                            "triangles": t.triangles,
                            "parts": t.parts,
                            "local": t.local,
                            "open": open.is_open(t.target),
                        })
                    })
                    .collect(),
            ))
        }
        m::GEOMETRY_OPEN => {
            let t = target(app, p)?;
            run(app, ModelAction::Open(t))?;
            let mesh = app.modeling.session.mesh(t).expect("opened");
            let (verts, edges, faces) = mesh.counts();
            // What the import joined or kept aside: coincident vertices
            // welded, and source triangles the mesh passes through
            // unedited (degenerate ones).
            let (welded, passthrough) = mesh.source().map_or((0, 0), |s| {
                (
                    s.positions.len().saturating_sub(verts),
                    s.triangles.len().saturating_sub(faces),
                )
            });
            Ok(json!({
                "target": t.id(),
                "verts": verts,
                "edges": edges,
                "faces": faces,
                "welded": welded,
                "passthrough": passthrough,
            }))
        }
        m::GEOMETRY_CLOSE => {
            let t = target(app, p)?;
            run(app, ModelAction::Close(t))?;
            Ok(json!({ "target": t.id(), "edited": app.modeling.session.is_open(t) }))
        }
        m::GEOMETRY_READ => {
            let t = open_target(app, p)?;
            let mesh = app.modeling.session.mesh(t).expect("open");
            let verts: Vec<Value> = mesh
                .vert_ids()
                .map(|v| json!({ "id": v.0, "position": mesh.position(v) }))
                .collect();
            let faces: Vec<Value> = mesh
                .face_ids()
                .map(|f| {
                    json!({
                        "id": f.0,
                        "verts": mesh.face_verts(f).iter().map(|v| v.0).collect::<Vec<_>>(),
                        "material": mesh.face_material(f),
                        "group": mesh.face_group(f),
                    })
                })
                .collect();
            let sel = mesh.selection();
            Ok(json!({
                "target": t.id(),
                "mode": mode_name(sel.mode()),
                "verts": verts,
                "faces": faces,
                "selected": sel.verts().map(|v| v.0).collect::<Vec<_>>(),
            }))
        }
        m::GEOMETRY_CHECK => {
            let only = match p.opt_str("target")? {
                Some(_) => Some(target(app, p)?),
                None => None,
            };
            let g = &app.modeling.session;
            let targets: Vec<Value> = g
                .check(only)
                .iter()
                .map(|c| {
                    json!({
                        "target": c.target.id(),
                        "class": c.class.name(),
                        "verts": c.verts,
                        "faces": c.faces,
                        "moved": c.moved,
                        "lightmap_stale": c.lightmap_stale,
                        "ops": c.ops,
                        "refused": c.refused,
                    })
                })
                .collect();
            let save_refusal = app.geometry_save_refusal();
            Ok(json!({
                "targets": targets,
                "allow_unverified": app.modeling.allow_unverified,
                "save_refusal": save_refusal,
            }))
        }
        m::MODEL_COLLISION_FROM_RENDER => {
            let target = open_target(app, p)?;
            let render = parse_target("render", p.str("render")?)?;
            let materials = p
                .get("materials")
                .map(|value| {
                    serde_json::from_value(value.clone())
                        .map_err(|e| err(code::INVALID_PARAMS, format!("materials: {e}")))
                })
                .transpose()?
                .unwrap_or_default();
            if !app.modeling.session.is_open(render) {
                return Err(err(
                    code::NO_DOCUMENT,
                    format!("{render} is not open (geometry.open)"),
                ));
            }
            app.perform(
                Action::Model(ModelAction::CollisionFromRender {
                    render,
                    target,
                    materials,
                }),
                false,
            )
            .map_err(|failure| match failure {
                Refused::Failed(reason) => err(code::REFUSED, reason),
                other => refused(other),
            })?;
            let (verts, _, faces) = app.modeling.session.mesh(target).expect("open target").counts();
            Ok(json!({ "target": target.id(), "verts": verts, "faces": faces }))
        }
        m::GEOMETRY_LOWER_LODS => {
            let t = open_target(app, p)?;
            let on = match p.get("on") {
                None | Some(Value::Null) => true,
                Some(_) => p.bool("on")?,
            };
            run(app, ModelAction::Seal)?;
            let was = app.modeling.session.uses_for_lower_lods(t);
            run(app, ModelAction::UseForLowerLods { target: t, on })?;
            Ok(json!({ "target": t.id(), "on": on, "changed": was != on }))
        }
        m::GEOMETRY_COLLISION_ADD_BOX => {
            let structure = parse_tag(p.str("structure")?)?;
            let (Some(lo), Some(hi)) = (p.opt_vec3("lo")?, p.opt_vec3("hi")?) else {
                return Err(err(code::INVALID_PARAMS, "lo and hi: the box's corners, [x,y,z] each"));
            };
            let material = match p.opt_u64("material")? {
                None => 0,
                Some(m) => u16::try_from(m)
                    .map_err(|_| err(code::INVALID_PARAMS, format!("material: {m} is past a u16")))?,
            };
            run(
                app,
                ModelAction::AddCollisionBox {
                    structure,
                    lo,
                    hi,
                    material,
                },
            )?;
            let edit = app
                .modeling
                .level_collision
                .get(&structure)
                .ok_or_else(|| err(code::FAILED, "the box was not added"))?;
            let level = &edit.added.level;
            Ok(json!({
                "structure": format!("{:#010x}", structure.0),
                "surfaces": level.bsp.surfaces.len(),
                "planes": level.bsp.planes.len(),
                "leaves": level.bsp.leaves.len(),
                "file_surfaces": edit.source.level.bsp.surfaces.len(),
            }))
        }
        m::GEOMETRY_COLLISION_CLEAR => {
            let structure = parse_tag(p.str("structure")?)?;
            let cleared = app.modeling.level_collision.contains_key(&structure);
            run(app, ModelAction::ClearCollision { structure })?;
            Ok(json!({ "structure": format!("{:#010x}", structure.0), "cleared": cleared }))
        }
        m::MESH_MODE => {
            let t = open_target(app, p)?;
            let mode = match p.str("mode")? {
                "vert" => SelectMode::Vert,
                "edge" => SelectMode::Edge,
                "face" => SelectMode::Face,
                other => {
                    return Err(err(
                        code::INVALID_PARAMS,
                        format!("mode: {other:?} is not vert, edge or face"),
                    ));
                }
            };
            apply(app, t, vec![MeshOp::Mode { mode }], None)?;
            Ok(json!({ "target": t.id(), "mode": mode_name(mode) }))
        }
        m::MESH_SELECT => {
            let t = open_target(app, p)?;
            let op = if p.bool("all")? {
                MeshOp::SelectAll
            } else if p.bool("none")? {
                MeshOp::SelectNone
            } else if p.bool("invert")? {
                MeshOp::SelectInvert
            } else {
                let ids: Vec<u32> = match p.get("ids") {
                    Some(v) => serde_json::from_value(v.clone())
                        .map_err(|e| err(code::INVALID_PARAMS, format!("ids: {e}")))?,
                    None => {
                        return Err(err(code::INVALID_PARAMS, "give ids, all, none or invert"));
                    }
                };
                let how = match p.opt_str("op")?.unwrap_or("set") {
                    "set" => SelectHow::Set,
                    "add" => SelectHow::Add,
                    "remove" => SelectHow::Remove,
                    "toggle" => SelectHow::Toggle,
                    other => {
                        return Err(err(
                            code::INVALID_PARAMS,
                            format!("op: {other:?} is not set, add, remove or toggle"),
                        ));
                    }
                };
                MeshOp::Select { ids, how }
            };
            apply(app, t, vec![op], None)?;
            let selected = app
                .modeling
                .session
                .mesh(t)
                .map_or(0, |m| m.selection().len());
            Ok(json!({ "target": t.id(), "selected": selected }))
        }
        m::MESH_TRANSFORM => {
            let t = open_target(app, p)?;
            let matrix = match (p.opt_vec3("translate")?, p.get("matrix")) {
                (Some(d), None) => [
                    [1.0, 0.0, 0.0, d[0]],
                    [0.0, 1.0, 0.0, d[1]],
                    [0.0, 0.0, 1.0, d[2]],
                ],
                (None, Some(v)) => serde_json::from_value(v.clone())
                    .map_err(|e| err(code::INVALID_PARAMS, format!("matrix: {e}")))?,
                _ => {
                    return Err(err(
                        code::INVALID_PARAMS,
                        "give translate or matrix, not both",
                    ));
                }
            };
            let pivot = p.opt_vec3("pivot")?;
            let merge = p.opt_u64("merge")?;
            apply(app, t, vec![MeshOp::Transform { matrix, pivot }], merge)?;
            Ok(summary(app, t))
        }
        m::MESH_OPS => {
            let t = open_target(app, p)?;
            let ops = serde_json::to_value(app.modeling.session.ops(t))
                .map_err(|e| err(code::FAILED, e.to_string()))?;
            Ok(json!({ "target": t.id(), "ops": ops }))
        }
        m::MESH_APPLY => {
            let t = open_target(app, p)?;
            let ops: Vec<MeshOp> =
                serde_json::from_value(p.get("ops").cloned().unwrap_or(Value::Null))
                    .map_err(|e| err(code::INVALID_PARAMS, format!("ops: {e}")))?;
            let n = ops.len();
            apply(app, t, ops, None)?;
            let mut v = summary(app, t);
            v["ops"] = json!(n);
            Ok(v)
        }
        _ => unreachable!("listed in METHODS"),
    }
}

fn mode_name(m: SelectMode) -> &'static str {
    match m {
        SelectMode::Vert => "vert",
        SelectMode::Edge => "edge",
        SelectMode::Face => "face",
    }
}

fn parse_tag(s: &str) -> Result<TagId, RpcError> {
    u32::from_str_radix(s.strip_prefix("0x").unwrap_or(s), 16)
        .map(TagId)
        .map_err(|_| {
            err(
                code::INVALID_PARAMS,
                format!("{s:?} is not a tag id"),
            )
        })
}
