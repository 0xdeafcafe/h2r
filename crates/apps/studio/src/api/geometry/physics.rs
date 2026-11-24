use serde_json::{Value, json};
use studio_proto::{code, method as m};
use tags::collision::{ShapeType, Transform, shape_edit::ShapeEdit};

use super::super::tag_fields;
use super::{MethodDef, P, Reply, def, err};
use crate::{App, app::TagAction};

pub(super) const SHAPES: MethodDef = def(
    m::MODEL_PHYSICS_SHAPES,
    "Read editable physics primitives and hulls from the open map's phmo tag, including unsaved edits",
    r#"{"tag":"string|integer"}"#,
    r#"{"spheres":"[{radius,translation,rotation}]","pills":"[{radius,bottom,top}]","boxes":"[{half_extents,radius,translation,rotation}]","polyhedra":"[{vertices,center,half_extents,planes}]"}"#,
    r#"{"tag":"phmo"}"#,
);
pub(super) const SET: MethodDef = def(
    m::MODEL_PHYSICS_SET,
    "Set one physics shape field in place as one map undo step. A vertex edit must preserve the hull's convex faces. Indices start at zero",
    r#"{"tag":"string|integer","kind":"sphere|pill|box|polyhedron","index":"integer","field":"radius|half_extents|bottom|top|translation|rotation|vertex","value":"number|[x,y,z]|[[x,y,z],[x,y,z],[x,y,z]]","vertex?":"integer (required for field vertex)"}"#,
    r#"{"warning":"string|null"}"#,
    r#"{"tag":"phmo","kind":"sphere","index":0,"field":"radius","value":1.25}"#,
);

pub(super) fn call(app: &mut App, method: &str, p: &P) -> Reply {
    if app.browsing().is_some() {
        return Err(err(
            code::REFUSED,
            "open the map in Studio to use its physics model",
        ));
    }
    let tag = tag_fields::resolve(app, p)?;
    if method == m::MODEL_PHYSICS_SET {
        tag_fields::no_target(app)?;
        let invalid = |what| err(code::INVALID_PARAMS, what);
        let kind = match p.str("kind")? {
            "sphere" => ShapeType::Sphere,
            "pill" => ShapeType::Pill,
            "box" => ShapeType::Box,
            "polyhedron" => ShapeType::Polyhedron,
            _ => return Err(invalid("kind must be sphere, pill, box or polyhedron")),
        };
        let vec = || {
            p.opt_vec3("value")?
                .ok_or_else(|| invalid("value is required"))
        };
        let edit = match p.str("field")? {
            "radius" => ShapeEdit::Radius(
                p.opt_f32("value")?
                    .ok_or_else(|| invalid("value is required"))?,
            ),
            "half_extents" => ShapeEdit::HalfExtents(vec()?),
            "bottom" => ShapeEdit::Bottom(vec()?),
            "top" => ShapeEdit::Top(vec()?),
            "translation" => ShapeEdit::Translation(vec()?),
            "rotation" => ShapeEdit::Rotation(
                serde_json::from_value(p.get("value").cloned().unwrap_or(Value::Null))
                    .map_err(|_| invalid("value must be three rotation rows"))?,
            ),
            "vertex" => ShapeEdit::Vertex {
                index: p.usize("vertex")?,
                position: vec()?,
            },
            _ => return Err(invalid("unknown physics shape field")),
        };
        let before = app.tags.undo_len();
        tag_fields::perform(
            app,
            TagAction::SetPhysicsShape {
                tag,
                kind,
                index: p.usize("index")?,
                edit,
            },
        )?;
        let warning = (app.tags.undo_len() != before)
            .then_some("Rigid-body bounds and mass properties may be stale.");
        return Ok(json!({"warning": warning}));
    }
    let model = app.physics_model(tag).map_err(|e| err(code::REFUSED, e))?;
    let transformed = |radius: f32, transform: Transform| json!({"radius": radius, "rotation": transform.rotation, "translation": transform.translation});
    Ok(json!({
        "spheres": model.spheres.iter().map(|s| transformed(s.radius, s.transform)).collect::<Vec<_>>(),
        "pills": model.pills.iter().map(|s| json!({"radius": s.radius, "bottom": s.bottom, "top": s.top})).collect::<Vec<_>>(),
        "boxes": model.boxes.iter().map(|s| {
            let mut value = transformed(s.radius, s.transform);
            value["half_extents"] = json!(s.half_extents);
            value
        }).collect::<Vec<_>>(),
        "polyhedra": model.polyhedra.iter().map(|h| json!({
            "vertices": h.vertices, "center": h.aabb_center, "half_extents": h.aabb_half_extents, "planes": h.planes
        })).collect::<Vec<_>>(),
    }))
}
