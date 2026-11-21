//! `collision.*`: the open map's level collision surfaces, classified as
//! drawn or collision only ([`scenario_edit::collision_view`];
//! docs/editor/polish-plan.md, slice P10). Read only, and independent of
//! the layers shown: an agent can ask about a surface the viewport hides.
//! `collision.objects` lists the placed objects' `coll` and `phmo` shapes
//! ([`scenario_edit::object_collision`], slice P11).

use agent_proto::RpcError;
use scenario_edit::collision_view::{CollisionSurface, LevelCollision, SurfaceClass};
use scenario_edit::geometry::{Ray, normalize};
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::scenario::{open, refused, tag_id};
use super::{P, Reply, err};
use crate::App;
use crate::app::Action;

/// How far (world units) from a surface's plane `collision.at`'s point
/// may be by default.
const AT_WITHIN: f32 = 0.05;

/// The open map's level collision, or NO_DOCUMENT.
fn collision(app: &App) -> Result<&LevelCollision, RpcError> {
    open(app)?;
    app.session
        .collision()
        .ok_or_else(|| err(code::NO_DOCUMENT, "no map is open"))
}

/// One surface as the API reports it.
pub fn surface_row(index: usize, s: &CollisionSurface) -> Value {
    json!({
        "index": index,
        "bsp": s.bsp,
        "surface": s.surface,
        "instance": s.instance,
        "class": s.class.name(),
        "plane": s.plane,
        "flags": s.flags,
        "breakable": s.flags & 0x08 != 0,
        "breakable_surface": (s.breakable != 0xff).then_some(s.breakable),
        "material": s.material,
        "covered": s.covered,
        "area": s.area(),
        "center": s.center(),
        "ring": s.ring,
    })
}

fn class_param(p: &P) -> Result<Option<SurfaceClass>, RpcError> {
    p.opt_str("class")?
        .map(|name| {
            SurfaceClass::from_name(name).ok_or_else(|| {
                err(
                    code::INVALID_PARAMS,
                    format!("class: drawn or collision_only, not {name:?}"),
                )
            })
        })
        .transpose()
}

pub fn call(app: &mut App, method: &str, p: &P) -> Reply {
    match method {
        m::COLLISION_SURFACES => {
            let class = class_param(p)?;
            let bsp = p.opt_usize("bsp")?;
            let offset = p.opt_usize("offset")?.unwrap_or(0);
            let limit = p.opt_usize("limit")?.unwrap_or(200);
            let c = collision(app)?;
            let matching: Vec<(usize, &CollisionSurface)> = c
                .surfaces
                .iter()
                .enumerate()
                .filter(|(_, s)| class.is_none_or(|k| s.class == k))
                .filter(|(_, s)| bsp.is_none_or(|b| s.bsp == b))
                .collect();
            let rows: Vec<Value> = matching
                .iter()
                .skip(offset)
                .take(limit)
                .map(|&(i, s)| surface_row(i, s))
                .collect();
            Ok(json!({
                "bsps": c.bsps.iter().map(|&id| tag_id(id)).collect::<Vec<_>>(),
                "counts": {
                    "drawn": c.count(SurfaceClass::Drawn),
                    "collision_only": c.count(SurfaceClass::CollisionOnly),
                },
                "matching": matching.len(),
                "surfaces": rows,
                "selected": app.session.selected_surface(),
                "errors": c.errors.iter()
                    .map(|(bsp, e)| json!({"bsp": bsp, "error": e}))
                    .collect::<Vec<_>>(),
            }))
        }
        m::COLLISION_AT => {
            let c = collision(app)?;
            let hit = match (
                p.opt_vec3("origin")?,
                p.opt_vec3("dir")?,
                p.opt_vec3("point")?,
            ) {
                (Some(origin), Some(dir), None) => {
                    if dir.iter().all(|&v| v == 0.0) {
                        return Err(err(code::INVALID_PARAMS, "dir must not be zero"));
                    }
                    let ray = Ray {
                        origin,
                        dir: normalize(dir),
                    };
                    c.pick(&ray, |_| true)
                }
                (None, None, Some(point)) => {
                    let within = p.opt_f32("within")?.unwrap_or(AT_WITHIN);
                    c.at_point(point, within.max(0.0))
                }
                _ => {
                    return Err(err(
                        code::INVALID_PARAMS,
                        "give a ray (origin and dir) or a point, not both",
                    ));
                }
            };
            let row = hit.map(|(d, i)| (d, surface_row(i, &c.surfaces[i])));
            if p.bool("select")? {
                let index = hit.map(|(_, i)| i);
                app.perform(Action::SelectSurface(index), false)
                    .map_err(refused)?;
            }
            Ok(match row {
                Some((distance, surface)) => json!({
                    "surface": surface,
                    "distance": distance,
                    "selected": app.session.selected_surface(),
                }),
                None => json!({
                    "surface": null,
                    "distance": null,
                    "selected": app.session.selected_surface(),
                }),
            })
        }
        m::COLLISION_OBJECTS => {
            let block = p
                .opt_str("block")?
                .map(super::params::block_kind)
                .transpose()?;
            let index = p.opt_usize("index")?;
            collision(app)?;
            let objects = app
                .session
                .object_collisions()
                .ok_or_else(|| err(code::NO_DOCUMENT, "no map is open"))?;
            let models: Vec<Value> = objects
                .models()
                .into_iter()
                .map(|(id, c)| {
                    json!({
                        "object": tag_id(id),
                        "resolved": c.object.map(tag_id),
                        "model": c.model.map(tag_id),
                        "coll": c.coll.map(tag_id),
                        "phmo": c.phmo.map(tag_id),
                        "nodes": c.nodes,
                        "shapes": c.shapes.len(),
                        "errors": c.errors,
                    })
                })
                .collect();
            let shapes: Vec<Value> = app
                .session
                .placed_object_shapes()
                .into_iter()
                .filter(|s| block.is_none_or(|b| s.marker.block == b))
                .filter(|s| index.is_none_or(|i| s.marker.index == i))
                .map(|s| {
                    json!({
                        "block": super::params::block_name(s.marker.block),
                        "index": s.marker.index,
                        "object": s.marker.object.map(tag_id),
                        "kind": s.shape.kind.name(),
                        "tag": tag_id(s.shape.tag),
                        "shape": s.shape.index,
                        "node": s.shape.node,
                        "center": s.center,
                        "radius": s.radius,
                        "edges": s.lines.len(),
                    })
                })
                .collect();
            Ok(json!({
                "models": models,
                "shapes": shapes,
                "error": objects.error,
            }))
        }
        m::COLLISION_SELECT => {
            let count = collision(app)?.surfaces.len();
            let index = p.opt_usize("index")?;
            if let Some(i) = index.filter(|&i| i >= count) {
                return Err(err(
                    code::INVALID_PARAMS,
                    format!("no collision surface {i} (the map has {count})"),
                ));
            }
            app.perform(Action::SelectSurface(index), false)
                .map_err(refused)?;
            if index.is_some() && p.bool("focus")? {
                app.perform(Action::FocusSelected, false).map_err(refused)?;
            }
            Ok(json!({ "selected": app.session.selected_surface() }))
        }
        _ => Err(err(code::METHOD_NOT_FOUND, format!("no method {method}"))),
    }
}
