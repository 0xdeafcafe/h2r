//! Model poses as the agent API ([`super::Provider`], registered in
//! [`super::seams`]): `anim.list` and `anim.pose` over the Tags tool's
//! own model poses (`app::anim`), so the Animation window and the API are
//! one path. docs/editor/agent-api.md, docs/editor/rich-editors.md
//! ("Model poses", slice M1).

use std::path::PathBuf;

use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::models::size;
use super::view::Gpu;
use super::{MethodDef, P, Provider, Reply, def, err};
use crate::App;
use crate::app::anim::{AnimError, AnimInfo, PoseAnswer};

const LIST: MethodDef = def(
    m::ANIM_LIST,
    "A jmad tag of the map the Tags tool shows: every animation with its name, type (base, overlay or replacement), frame count, the codecs of its static and animated streams, whether it is an aim grid (codec 8, whose frames are aim cells, not time) and why any clip does not build; and the map's objects whose hlmt names the graph",
    r#"{"tag":"string"}"#,
    r#"{"tag":"string","nodes":"integer","animations":"[{index,name,type,frames,codecs:[string|null],aim_grid,error}]","users":"[{object,name}]"}"#,
    r#"{"tag":"jmad:invented\\stickman"}"#,
);
const POSE: MethodDef = def(
    m::ANIM_POSE,
    "One animation of a jmad tag sampled at a frame (a fractional frame blends the neighbours) and posed on the model that uses the graph: every node's local transform (the animation's channel where it has one, the node's default where it does not), which channels the animation supplied, and the posed world matrix. `overlay` applies a second animation over the base at a weight with the engine's scaled-add rule. The answer also draws the posed model offscreen through the viewport's renderer to a PNG (default: the cache directory's studio/anim/; never into a Git checkout); without a renderer, or when no model of the map uses the graph, the transforms still answer and a note says so",
    r#"{"tag":"string","animation":"string|integer (its name or index)","frame?":"number (default 0)","overlay?":{"animation":"string|integer","weight":"number (0 to 1)"},"model?":"string (an object that uses the graph, by name or 0x... id; default its only one)","size?":"[w,h] (default [640,480])","out?":"string"}"#,
    r#"{"tag":"string","animation":"string","frame":"number","frames":"integer","model":"{object,name,triangles,untextured}|null","nodes":"[{index,name,parent,rotation,translation,scale,has:{rotation,translation,scale},world:[[f,f,f,f];4]|null}]","png":"string|null","width":"integer","height":"integer","covered":"integer","notes":"[string]"}"#,
    r#"{"tag":"jmad:invented\\stickman","animation":"combat:idle","frame":2,"out":"$OUT/pose.png"}"#,
);

fn failed(e: AnimError) -> agent_proto::RpcError {
    match e {
        AnimError::Refused(m) => err(code::REFUSED, m),
        AnimError::Failed(m) => err(code::FAILED, m),
    }
}

/// The `tag` param as an animation graph of the document the Tags tool
/// shows.
fn anim_tag(app: &mut App, p: &P) -> Result<map::TagId, agent_proto::RpcError> {
    super::tag_fields::resolve(app, p)
}

/// The `animation` param (`want`) as an index into `info`'s list: a name
/// or the index itself.
fn find_animation(info: &AnimInfo, want: &Value) -> Result<usize, agent_proto::RpcError> {
    let animations = &info.animations;
    let found = match want {
        Value::String(s) => animations.iter().position(|a| &a.name == s),
        Value::Number(n) => n
            .as_u64()
            .and_then(|i| usize::try_from(i).ok())
            .filter(|&i| i < animations.len()),
        _ => None,
    };
    match found {
        Some(i) => Ok(i),
        None => {
            let names: Vec<&str> = animations.iter().map(|a| a.name.as_str()).collect();
            Err(err(
                code::INVALID_PARAMS,
                format!(
                    "animation: {want:?} (it has: {})",
                    if names.is_empty() {
                        "no animations".to_owned()
                    } else {
                        names.join(", ")
                    }
                ),
            ))
        }
    }
}

fn summary_json(a: &crate::app::anim::AnimSummary) -> Value {
    json!({
        "index": a.index,
        "name": a.name,
        "type": a.kind,
        "frames": a.frames,
        "codecs": a.codecs.map(|c| c.map(|x| x.name())),
        "aim_grid": a.aim_grid,
        "error": a.error,
    })
}

fn user_json(u: &crate::app::anim::ModelUser) -> Value {
    json!({
        "object": format!("{:#010x}", u.object.0),
        "name": u.name,
    })
}

fn answer_json(answer: &PoseAnswer, picture: (&Value, u32, u32, usize)) -> Value {
    let (png, width, height, covered) = picture;
    let model = answer.model.as_ref().map(|m| {
        json!({
            "object": format!("{:#010x}", m.object.0),
            "name": m.name,
            "triangles": m.piece.triangles(),
            "untextured": m.piece.untextured.len(),
        })
    });
    let nodes: Vec<Value> = answer
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            json!({
                "index": i,
                "name": n.name,
                "parent": n.parent,
                "rotation": n.rotation,
                "translation": n.translation,
                "scale": n.scale,
                "has": {
                    "rotation": n.has[0],
                    "translation": n.has[1],
                    "scale": n.has[2],
                },
                // Column-major T·R·S: the outer array is the columns.
                "world": n.world.map(|w| {
                    [w[0].to_vec(), w[1].to_vec(), w[2].to_vec(), w[3].to_vec()]
                }),
            })
        })
        .collect();
    json!({
        "animation": summary_json(&answer.animation),
        "frame": answer.frame,
        "frames": answer.animation.frames,
        "model": model,
        "nodes": nodes,
        "png": png,
        "width": width,
        "height": height,
        "covered": covered,
        "notes": answer.notes,
    })
}

/// The provider: a GPU for the renders, held between calls.
#[derive(Default)]
pub struct Anim {
    gpu: Option<Result<Gpu, String>>,
}

impl Provider for Anim {
    fn methods(&self) -> Vec<MethodDef> {
        vec![LIST, POSE]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        if !matches!(method, m::ANIM_LIST | m::ANIM_POSE) {
            return None;
        }
        let p = P(params);
        Some(match method {
            m::ANIM_LIST => self.list(app, &p),
            _ => self.pose(app, &p),
        })
    }
}

impl Anim {
    fn list(&mut self, app: &mut App, p: &P) -> Reply {
        let id = anim_tag(app, p)?;
        let info = app.anim_info(id).map_err(failed)?;
        Ok(json!({
            "tag": p.str("tag")?,
            "nodes": info.nodes,
            "animations": info.animations.iter().map(summary_json).collect::<Vec<_>>(),
            "users": info.users.iter().map(user_json).collect::<Vec<_>>(),
        }))
    }

    fn pose(&mut self, app: &mut App, p: &P) -> Reply {
        let id = anim_tag(app, p)?;
        let info = app.anim_info(id).map_err(failed)?;
        let animation = find_animation(&info, p.get("animation").unwrap_or(&Value::Null))?;
        let frame = p.opt_f32("frame")?.unwrap_or(0.0);
        let overlay = match p.get("overlay") {
            None => None,
            Some(v) => {
                let o = v.as_object().ok_or_else(|| {
                    err(code::INVALID_PARAMS, format!("overlay: an object, not {v}"))
                })?;
                let bad = || {
                    err(
                        code::INVALID_PARAMS,
                        "overlay: {animation (name or index), weight (0 to 1)}".to_owned(),
                    )
                };
                let want = o.get("animation").ok_or_else(bad)?;
                let weight = o
                    .get("weight")
                    .and_then(Value::as_f64)
                    .filter(|f| f.is_finite())
                    .ok_or_else(bad)? as f32;
                if !(0.0..=1.0).contains(&weight) {
                    return Err(err(
                        code::INVALID_PARAMS,
                        format!("overlay.weight: 0 to 1, not {weight}"),
                    ));
                }
                Some((find_animation(&info, want)?, weight))
            }
        };
        let model = p.opt_str("model")?;
        let mut answer = app
            .anim_pose(id, animation, frame, overlay, model)
            .map_err(failed)?;
        let size = size(p)?;
        let out = match p.opt_str("out")? {
            Some(o) => PathBuf::from(o),
            None => {
                let stem = p
                    .str("tag")?
                    .rsplit(['\\', '/'])
                    .next()
                    .unwrap_or("pose")
                    .to_owned();
                studio_proto::output_dir().join("anim").join(format!(
                    "{stem}_{}_{}.png",
                    answer.animation.name, answer.frame
                ))
            }
        };
        // Everything but the picture answers on any machine; the picture
        // says why it is not there.
        let mut notes = answer.notes.clone();
        let picture = match &answer.model {
            Some(m) => match self.picture(m, &out, size) {
                Ok((width, height, covered)) => {
                    (json!(out.display().to_string()), width, height, covered)
                }
                Err(e) => {
                    notes.push(e);
                    (Value::Null, 0, 0, 0)
                }
            },
            None => (Value::Null, 0, 0, 0),
        };
        answer.notes = notes;
        Ok(answer_json(&answer, picture))
    }

    /// The posed model drawn to `out`: the picture's size and how many
    /// pixels it covers; an error names why there is none.
    fn picture(
        &mut self,
        model: &crate::app::anim::PoseModel,
        out: &std::path::Path,
        size: [u32; 2],
    ) -> Result<(u32, u32, usize), String> {
        let gpu = self
            .gpu
            .get_or_insert_with(|| Gpu::new().map_err(|e| format!("{e:#}")));
        let gpu = match gpu {
            Ok(g) => g,
            Err(e) => return Err(format!("no renderer here: {e}")),
        };
        super::docs::outside_git(out).map_err(|e| e.to_string())?;
        let (rgba, width, height) = crate::app::anim::draw(&gpu.device, &gpu.queue, model, size)?;
        if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        crate::headless::write_png(out, &rgba, width, height)
            .map_err(|e| format!("{}: {e:#}", out.display()))?;
        let covered =
            scenario_edit::native_view::preview::covered(&rgba, crate::app::anim::BACKGROUND);
        Ok((width, height, covered))
    }
}

#[cfg(test)]
#[path = "anim/tests.rs"]
pub(super) mod tests;
