//! The Shader tab as the agent API ([`super::Provider`], registered in
//! [`super::seams`]): `shader.read`, `shader.set`, `shader.preview` and
//! `shader.code` over `app::shader`, the tab's own read and draw. `shader.set` is
//! `tag.set` at the slot's path (`app::shader::SlotKind::path`), so the
//! write, its undo step and its refusals are the field panel's.
//! docs/editor/rich-editors.md, "Shaders"; docs/editor/agent-api.md.

use std::path::PathBuf;

use map::TagId;
use scenario_edit::native_view::code::{self as programs, ProgramCode};
use scenario_edit::native_view::preview::{Shape, covered};
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::view::Gpu;
use super::{MethodDef, P, Provider, Reply, def, err, tag_fields};
use crate::App;
use crate::app::shader::{self as model, ShaderRead, Slot, SlotKind, SlotValue};
use crate::app::tag_api;

/// The shape of a slot row, as `shader.read` lists them and `shader.set`
/// returns one.
const SLOT: &str = r#"{"kind":"bitmap|colour|vector","index":"integer","path":"string","label":"string","value":"as tag.read gives the field","bitmap?":"string","image?":"integer","hex?":"string","vector?":"[number]","used_by":"[{layer, pass, implementation, reads}]"}"#;

const READ: MethodDef = def(
    m::SHADER_READ,
    "A shader instance (shad) as the Shader tab shows it, read with the document's edits: its template, the level of detail shown (0) and how many the template has, its bitmap, colour and vector slots (the compiled postprocess the engine binds; each with its tag.set path and `used_by`, where the template's binding puts it: a texture stage, a combiner constant, a vertex default or constant, per pass and implementation), its animated properties and what each drives, the passes per layer, `names_known` (false: the template's parameter names are not read, so slots are named by kind and index), why it is read-only, and what did not read",
    r#"{"tag":"string|integer"}"#,
    r#"{"tag":"string","template":"string","lod":"integer","lods":"integer","bitmaps":"[slot]","colours":"[slot]","vectors":"[slot]","animations":"[{index, input, period, function_bytes, drives:[string]}]","passes":"{layer: count}","used_by":"{bitmap|colour|vector: [[used_by]]}","names_known":"boolean","names":"string|null","read_only":"string|null","refusals":"[string]"}"#,
    r#"{"tag":"shad:levels\\test\\shaders\\floor"}"#,
);
const SET: MethodDef = def(
    m::SHADER_SET,
    "Write one slot of a shader: `tag.set` at the slot's path with the same value (a bitmap's `\"bitm:name\"` or null, a colour's `\"#aarrggbb\"`, `\"#rrggbb\"` or `{rgba}`, a vector's four numbers), one undo step, refused as tag.set refuses (a read-only tag, a tag of the wrong group, a value that does not fit). Returns the slot as shader.read lists it",
    r#"{"tag":"string|integer","slot":"{kind: bitmap|colour|vector, index: integer}","value":"as tag.set takes for the field"}"#,
    SLOT,
    r##"{"tag":"shad:levels\\test\\shaders\\floor","slot":{"kind":"colour","index":0},"value":"#ffff0000"}"##,
);
const PREVIEW: MethodDef = def(
    m::SHADER_PREVIEW,
    "Draw a shader alone through the native renderer, the viewport's Game mode with no game: on a sphere (the default), a flat square, or an object tag's model in its rest pose, read with the document's edits, written to a PNG outside Git. `passes_drawn` counts the passes drawn; each layer whose passes are left out is in `refusals` with its pass count and why (layer 1 until its stencil mode is checked, a program that does not compile, an animated constant, fog, a view constant the editor does not give); `base` says why the base look does not draw, which the layers draw over. With no renderer the counts still answer and `png` is null",
    r#"{"tag":"string|integer","shape?":"sphere|quad|object","object?":"string|integer (the object tag, for shape object)","size?":"[w, h] (default [320, 240], each 16 to 2048)","out?":"string (a .png path outside Git)"}"#,
    r#"{"png":"string|null","width":"integer","height":"integer","covered":"integer","shape":"string","passes_drawn":"integer","refusals":"[{layer, passes, why}]","base":"string|null","unresolved":"[string]","notes":"[string]"}"#,
    r#"{"tag":"shad:levels\\test\\shaders\\floor","shape":"sphere","out":"$OUT/floor.png"}"#,
);

const CODE: MethodDef = def(
    m::SHADER_CODE,
    "The two programs of one pass of a shader as text, read with the document's edits: the vertex program (the world classification the preview draws) and the pixel program, each as its disassembly (`listing`), the WGSL the translator makes of it (`wgsl`) and the translator's `warnings`. `pass` counts the passes across the layers of the first level of detail, in layer order (default 0; `passes` is how many there are, `layer` and `pass_in_layer` where it sits); `implementation` picks one of the pass's implementations (default 0). A program that cannot be translated has its listing, a null `wgsl` and the refusal in `warnings` (a state program); one that does not decode has a null `listing` too. Nothing is written: the text is what the renderer is given, not a map edit",
    r#"{"tag":"string|integer","pass?":"integer (default 0)","implementation?":"integer (default 0)"}"#,
    r#"{"tag":"string","pass":"integer","passes":"integer","layer":"integer","pass_in_layer":"integer","pass_tag":"string|null","implementation":"integer","implementations":"integer","vertex":"{classification, listing: string|null, wgsl: string|null, warnings: [string]}","pixel":"{listing: string|null, wgsl: string|null, warnings: [string]}"}"#,
    r#"{"tag":"shad:levels\\test\\shaders\\floor","pass":0}"#,
);

/// The provider: a GPU for the previews, held between calls.
#[derive(Default)]
pub struct Shader {
    gpu: Option<Result<Gpu, String>>,
}

fn failed(e: String) -> agent_proto::RpcError {
    err(code::REFUSED, e)
}

/// A `tag:` name as the Tags tool writes it, or null.
fn tag_name(app: &App, id: TagId) -> Value {
    if id == TagId::NULL {
        return Value::Null;
    }
    let doc = app.shown_tags();
    doc.tag(id).map_or(Value::Null, |t| {
        json!(format!("{}:{}", tags::defs::fourcc_text(t.group), t.name))
    })
}

/// One slot as `shader.read` lists it.
pub(crate) fn slot_json(app: &App, tag: TagId, s: &Slot) -> Value {
    let path = s.path();
    let doc = app.shown_tags();
    let value = tag_api::resolve(doc, tag, &path)
        .map(|(at, _, f)| tag_api::value(doc, tag, at, &f))
        .unwrap_or(Value::Null);
    let mut out = json!({
        "kind": s.kind.id(),
        "index": s.index,
        "path": path,
        "label": s.label(),
        "value": value,
        "used_by": s.used_by.iter().map(model::UsedBy::json).collect::<Vec<_>>(),
    });
    match s.value {
        SlotValue::Bitmap { bitmap, image } => {
            out["bitmap"] = tag_name(app, bitmap);
            out["image"] = json!(image);
        }
        SlotValue::Colour(c) => out["hex"] = json!(format!("#{c:08x}")),
        SlotValue::Vector(v) => out["vector"] = json!(v),
    }
    out
}

fn read_json(app: &App, r: &ShaderRead) -> Value {
    let slots = |k: SlotKind| -> Vec<Value> {
        r.slots(k).iter().map(|s| slot_json(app, r.tag, s)).collect()
    };
    let used_by = |k: SlotKind| -> Vec<Value> {
        r.slots(k)
            .iter()
            .map(|s| json!(s.used_by.iter().map(model::UsedBy::json).collect::<Vec<_>>()))
            .collect()
    };
    let doc = app.shown_tags();
    json!({
        "tag": tag_name(app, r.tag),
        "template": tag_name(app, r.template),
        "lod": r.lod,
        "lods": r.lods,
        "bitmaps": slots(SlotKind::Bitmap),
        "colours": slots(SlotKind::Colour),
        "vectors": slots(SlotKind::Vector),
        "animations": r.animations.iter().map(|a| json!({
            "index": a.index,
            "input": a.input.map(|s| doc.string(s).map_or_else(|| json!(s), |t| json!(t))),
            "period": a.period,
            "function_bytes": a.function_bytes,
            "drives": a.drives,
        })).collect::<Vec<_>>(),
        "passes": r.passes.iter().map(|(l, n)| (l.to_string(), json!(n))).collect::<serde_json::Map<_, _>>(),
        "used_by": {
            "bitmap": used_by(SlotKind::Bitmap),
            "colour": used_by(SlotKind::Colour),
            "vector": used_by(SlotKind::Vector),
        },
        "names_known": r.names_known,
        "names": (!r.names_known).then_some(model::NAMES_NOT_READ),
        "read_only": r.read_only,
        "refusals": r.problems,
    })
}

fn read(app: &mut App, p: &P) -> Reply {
    let tag = tag_fields::resolve(app, p)?;
    let r = app.shader_read(tag).map_err(failed)?;
    Ok(read_json(app, &r))
}

/// The slot `shader.set` names.
fn slot_of(p: &P) -> Result<(SlotKind, usize), agent_proto::RpcError> {
    let bad = || err(code::INVALID_PARAMS, "slot: {kind: bitmap|colour|vector, index}");
    let s = p.get("slot").filter(|s| s.is_object()).ok_or_else(bad)?;
    let kind = s
        .get("kind")
        .and_then(Value::as_str)
        .and_then(SlotKind::from_id)
        .ok_or_else(bad)?;
    let index = s
        .get("index")
        .and_then(Value::as_u64)
        .and_then(|i| usize::try_from(i).ok())
        .ok_or_else(bad)?;
    Ok((kind, index))
}

fn set(app: &mut App, p: &P) -> Reply {
    let tag = tag_fields::resolve(app, p)?;
    let (kind, index) = slot_of(p)?;
    let before = app.shader_read(tag).map_err(failed)?;
    let n = before.slots(kind).len();
    if index >= n {
        return Err(err(
            code::INVALID_PARAMS,
            format!("the shader has {n} {} slots", kind.id()),
        ));
    }
    // tag.set itself: the same path, value, refusals and undo step.
    tag_fields::no_target(app)?;
    let path = kind.path(index);
    let value = p.get("value").cloned().unwrap_or(Value::Null);
    let action = tag_api::set_field(app.shown_tags(), tag, &path, &value)
        .map_err(|e| err(code::INVALID_PARAMS, format!("{path}: {e}")))?;
    tag_fields::perform(app, action)?;
    let after = app.shader_read(tag).map_err(failed)?;
    let s = after
        .slots(kind)
        .get(index)
        .ok_or_else(|| failed(format!("{} {index} did not read back", kind.id())))?;
    Ok(slot_json(app, tag, s))
}

/// The picture's size: `[w, h]`, each 16 to 2048.
fn size(p: &P) -> Result<[u32; 2], agent_proto::RpcError> {
    let Some(v) = p.get("size").filter(|v| !v.is_null()) else {
        return Ok([320, 240]);
    };
    match v.as_array().map(Vec::as_slice) {
        Some([w, h]) => match (w.as_u64(), h.as_u64()) {
            (Some(w), Some(h)) if (16..=2048).contains(&w) && (16..=2048).contains(&h) => {
                Ok([w as u32, h as u32])
            }
            _ => Err(err(code::INVALID_PARAMS, "size is [w, h], each 16 to 2048")),
        },
        _ => Err(err(code::INVALID_PARAMS, "size is [w, h]")),
    }
}

/// The shape `shader.preview` names.
fn shape_of(app: &mut App, p: &P) -> Result<Shape, agent_proto::RpcError> {
    match p.opt_str("shape")?.unwrap_or("sphere") {
        "sphere" => Ok(Shape::Sphere),
        "quad" | "square" => Ok(Shape::Quad),
        "object" => {
            let want = p
                .get("object")
                .cloned()
                .ok_or_else(|| err(code::INVALID_PARAMS, model::NO_OBJECT))?;
            let id = tag_api::lookup(app.shown_tags(), &want)
                .map_err(|e| err(code::INVALID_PARAMS, format!("object: {e}")))?;
            Ok(Shape::Object(id))
        }
        other => Err(err(
            code::INVALID_PARAMS,
            format!("shape: sphere, quad or object, not {other:?}"),
        )),
    }
}

fn program_json(p: &ProgramCode) -> Value {
    json!({"listing": p.listing, "wgsl": p.wgsl, "warnings": p.warnings})
}

fn read_code(app: &mut App, p: &P) -> Reply {
    let tag = tag_fields::resolve(app, p)?;
    let pass = p.opt_usize("pass")?.unwrap_or(0);
    let implementation = p.opt_usize("implementation")?.unwrap_or(0);
    let c = app
        .shader_code(tag, pass, implementation)
        .map_err(failed)?;
    let mut vertex = program_json(&c.vertex);
    vertex["classification"] = json!(programs::VERTEX_CLASSIFICATION);
    Ok(json!({
        "tag": tag_name(app, tag),
        "pass": c.pass,
        "passes": c.passes,
        "layer": c.layer,
        "pass_in_layer": c.pass_in_layer,
        "pass_tag": tag_name(app, c.pass_tag),
        "implementation": c.implementation,
        "implementations": c.implementations,
        "vertex": vertex,
        "pixel": program_json(&c.pixel),
    }))
}

impl Shader {
    fn preview(&mut self, app: &mut App, p: &P) -> Reply {
        let tag = tag_fields::resolve(app, p)?;
        let shape = shape_of(app, p)?;
        let size = size(p)?;
        let read = app.shader_read(tag).map_err(failed)?;
        let out = match p.opt_str("out")? {
            Some(o) => PathBuf::from(o),
            None => {
                let stem = tag_name(app, tag)
                    .as_str()
                    .and_then(|n| n.rsplit(['\\', '/', ':']).next())
                    .unwrap_or("shader")
                    .to_owned();
                studio_proto::output_dir()
                    .join("shader")
                    .join(format!("{stem}_{}.png", shape.id()))
            }
        };
        super::docs::outside_git(&out)?;
        let gpu = self
            .gpu
            .get_or_insert_with(|| Gpu::new().map_err(|e| format!("{e:#}")));
        let mut notes = Vec::new();
        let (png, width, height, covered_px, passes) = match gpu {
            Ok(g) => {
                let pic = app
                    .shader_picture(&g.device, &g.queue, &read, shape, size)
                    .map_err(failed)?;
                if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
                    std::fs::create_dir_all(dir)
                        .map_err(|e| failed(format!("{}: {e}", dir.display())))?;
                }
                crate::headless::write_png(&out, &pic.rgba, pic.width, pic.height)
                    .map_err(|e| failed(format!("{}: {e:#}", out.display())))?;
                let c = covered(&pic.rgba, model::BACKGROUND);
                (
                    json!(out.display().to_string()),
                    pic.width,
                    pic.height,
                    c,
                    pic.passes,
                )
            }
            Err(e) => {
                notes.push(format!("no renderer here: {e}"));
                let preview = app.shader_preview(tag, shape).map_err(failed)?;
                let draws = scenario_edit::native_view::preview::material_draws(&preview);
                (Value::Null, 0, 0, 0, model::PreviewPasses::of(&read, &draws))
            }
        };
        let mut answer = passes.json();
        answer["png"] = png;
        answer["width"] = json!(width);
        answer["height"] = json!(height);
        answer["covered"] = json!(covered_px);
        answer["shape"] = json!(shape.id());
        answer["notes"] = json!(notes);
        Ok(answer)
    }
}

impl Provider for Shader {
    fn methods(&self) -> Vec<MethodDef> {
        vec![READ, SET, PREVIEW, CODE]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        let p = P(params);
        Some(match method {
            m::SHADER_READ => read(app, &p),
            m::SHADER_SET => set(app, &p),
            m::SHADER_PREVIEW => self.preview(app, &p),
            m::SHADER_CODE => read_code(app, &p),
            _ => return None,
        })
    }
}
