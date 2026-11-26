//! `markers.*` (docs/game/gametype-markers.md, "Agent API"): the game type
//! markers of the open map as the viewport draws them (`markers.list`),
//! and the shapes themselves rendered on a grid with no map
//! (`markers.render`), the content render of the marker set.

use std::path::PathBuf;

use scenario_edit::geometry::{Camera, Gameplay, Tri};
use scenario_edit::layers::{LayerId, Layers};
use scenario_edit::marker_view::game_markers::{self, Area, Kind, Marker, Mesh, Palette, Shape, Tint};
use scenario_edit::marker_view::{self, Placed};
use scenario_edit::render::{self, Viewport};
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::view::Gpu;
use super::{MethodDef, P, Provider, Reply, err};
use crate::App;

/// The `markers.*` provider: a GPU for the renders.
#[derive(Default)]
pub struct Markers {
    gpu: Option<Result<Gpu, String>>,
}

const METHODS: &[MethodDef] = &[
    MethodDef {
        name: m::MARKERS_LIST,
        summary: "Every game type marker of the open map as the viewport draws it: spawns, bomb, flag, ball, teleporter and territory flags, hills (three or more corner flags of one hill are one marker with the area of their hull), item spawners and trigger volumes. `kind` (a name from markers.render's kinds, e.g. spawn, hill, trigger_volume) and `layer` (spawn_points, flags, netgame_items, trigger_volumes, kill_volumes, bsp_switches) narrow it. A flag of a type the game does not read has no marker",
        params: r#"{"kind?":"string","layer?":"string"}"#,
        result: r#"{"markers":"[{id,kind,layer,team,position,facing,label,number,area,area_from,members,bounds,lines,triangles}]","counts":"{kind: integer}"}"#,
        example: r#"{"kind":"spawn"}"#,
        planned: None,
    },
    MethodDef {
        name: m::MARKERS_RENDER,
        summary: "Draw the marker shapes offscreen with the viewport's renderer to a PNG, on a floor grid at the scale of a 0.7 figure (default: the cache directory's studio/markers/; never into a Git checkout). With no `kind`, the contact sheet of every kind in red, blue and neutral (a kind with one fixed colour once); `team` (0 to 7) draws one team. `area` ({radius, below?, above?} or {path: [[x,y],...], below?, above?}, numbers) replaces the area of a hill, territory or site. `view`: angled (default), top or front, with +x to the right. Needs no map and no disc",
        params: r#"{"kind?":"string","team?":"integer (0 to 7)","area?":"object","view?":"angled|top|front","size?":"[w,h] (default [1280,720])","out?":"string"}"#,
        result: r#"{"path":"string","width":"integer","height":"integer","view":"string","covered":"integer (pixels off the floor's background)","kinds":"[{kind,team,cell:[col,row],center:[x,y] (pixels),lines,triangles,bounds}]"}"#,
        example: r#"{"kind":"spawn","out":"$OUT/markers.png","size":[64,48]}"#,
        planned: None,
    },
    MethodDef {
        name: m::MARKERS_AREA_SET,
        summary: "Set or clear the area of a netgame flag (a hill, territory or site flag): one undo step. `id` is the flag as markers.list names it (netgame_flags[3]); `area` is {radius, below?, above?} or {path: [[x,y],...], below?, above?} in plain numbers (a path's points are offsets from the flag), or null to clear. Answers the area as stored (floats in hex, as the document writes it) or null. Refused with Area::validate's sentence, and for a flag that is a corner of a hill with corners",
        params: r#"{"id":"string","area":"object|null"}"#,
        result: r#"{"id":"string","area":"object|null"}"#,
        example: r#"{"id":"netgame_flags[0]","area":{"radius":4}}"#,
        planned: None,
    },
    MethodDef {
        name: m::MARKERS_AREA_CONVERT,
        summary: "Convert a retail hill's corner flags (three or more of one hill type and identifier) to one flag at the hull's centre with that hull as its path area, in one undo step. `id` is any corner. Answers the converted flag and its area",
        params: r#"{"id":"string"}"#,
        result: r#"{"id":"string","area":"object","position":"[x,y,z]"}"#,
        example: r#"{"id":"netgame_flags[0]"}"#,
        planned: None,
    },
];

impl Provider for Markers {
    fn methods(&self) -> Vec<MethodDef> {
        METHODS.to_vec()
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        let p = P(params);
        match method {
            m::MARKERS_LIST => Some(list(app, &p)),
            m::MARKERS_RENDER => Some(self.render(&p)),
            m::MARKERS_AREA_SET => Some(area_set(app, &p)),
            m::MARKERS_AREA_CONVERT => Some(area_convert(app, &p)),
            _ => None,
        }
    }
}

/// An area as the document writes it: floats as their bits in hex.
fn area_json(a: &Area) -> Value {
    let hex = |f: f32| format!("{:08x}", f.to_bits());
    match &a.shape {
        Shape::Radius(r) => json!({"radius": {
            "radius": hex(*r), "below": hex(a.below), "above": hex(a.above)}}),
        Shape::Box { half } => json!({"box": {
            "half": [hex(half[0]), hex(half[1])], "below": hex(a.below), "above": hex(a.above)}}),
        Shape::Sphere(r) => json!({"sphere": {"radius": hex(*r)}}),
        Shape::Path(points) => json!({"path": {
            "points": points.iter().map(|p| [hex(p[0]), hex(p[1])]).collect::<Vec<_>>(),
            "below": hex(a.below), "above": hex(a.above)}}),
    }
}

/// A flag's index from an id as markers.list names it.
fn flag_index(app: &App, id: &str) -> Result<usize, agent_proto::RpcError> {
    let scenario = super::scenario::open(app)?;
    let bad = || {
        err(
            code::INVALID_PARAMS,
            format!("id: a netgame flag as markers.list names it (netgame_flags[3]), not {id:?}"),
        )
    };
    let n = id
        .strip_prefix("netgame_flags[")
        .and_then(|r| r.strip_suffix(']'))
        .and_then(|n| n.parse::<usize>().ok())
        .ok_or_else(bad)?;
    if n >= scenario.block(scenario_edit::BlockKind::NetgameFlags).len() {
        return Err(err(
            code::INVALID_PARAMS,
            format!("id: the map has no {id}"),
        ));
    }
    Ok(n)
}

fn refused(e: scenario_edit::Error) -> agent_proto::RpcError {
    match e {
        scenario_edit::Error::Declined(why) => err(code::REFUSED, why),
        other => err(code::FAILED, other.to_string()),
    }
}

/// `markers.area.set`.
fn area_set(app: &mut App, p: &P) -> Reply {
    let id = p.str("id")?;
    let index = flag_index(app, id)?;
    // `null` clears; a missing `area` is an error, not a clear.
    let area = match p.0.get("area") {
        None => return Err(err(code::INVALID_PARAMS, "area is required (an object, or null to clear)")),
        Some(Value::Null) => None,
        Some(v) => Some(area_param(v)?),
    };
    app.session.set_area(index, area, None).map_err(refused)?;
    Ok(json!({
        "id": id,
        "area": app.session.flag_area(index).map(area_json),
    }))
}

/// `markers.area.convert`.
fn area_convert(app: &mut App, p: &P) -> Reply {
    let index = flag_index(app, p.str("id")?)?;
    let kept = app.session.convert_hill(index).map_err(refused)?;
    let position = app
        .session
        .markers()
        .iter()
        .find(|m| m.block == scenario_edit::BlockKind::NetgameFlags && m.index == kept)
        .map(|m| m.position);
    Ok(json!({
        "id": format!("netgame_flags[{kept}]"),
        "area": app.session.flag_area(kept).map(area_json),
        "position": position,
    }))
}

fn bounds_json(b: Option<([f32; 3], [f32; 3])>) -> Value {
    match b {
        Some((min, max)) => json!({"min": min, "max": max}),
        None => Value::Null,
    }
}

fn list(app: &App, p: &P) -> Reply {
    let scenario = super::scenario::open(app)?;
    let kind = p.opt_str("kind")?;
    if let Some(k) = kind
        && !Kind::samples().iter().any(|s| s.name() == k)
    {
        return Err(unknown_kind(k));
    }
    let layer = p.opt_str("layer")?;
    if let Some(l) = layer
        && !marker_view::layers().any(|id| id.id() == l)
    {
        let names: Vec<_> = marker_view::layers().map(LayerId::id).collect();
        return Err(err(
            code::INVALID_PARAMS,
            format!("layer: one of {}, not {l:?}", names.join(", ")),
        ));
    }
    let placed = placed_now(app, scenario);
    let palette = Palette::default();
    let mut counts = serde_json::Map::new();
    let rows: Vec<Value> = placed
        .iter()
        .filter(|x| kind.is_none_or(|k| x.marker.kind.name() == k))
        .filter(|x| layer.is_none_or(|l| x.layer.id() == l))
        .map(|x| {
            let mesh = x.mesh(&palette, false);
            let n = counts.entry(x.marker.kind.name().to_owned()).or_insert(json!(0));
            *n = json!(n.as_u64().unwrap_or(0) + 1);
            row(x, &mesh)
        })
        .collect();
    Ok(json!({"markers": rows, "counts": counts}))
}

/// Every marker of the open map as the viewport draws it: the document's
/// areas included.
pub(super) fn placed_now(app: &App, scenario: &scenario_edit::Scenario) -> Vec<Placed> {
    let session = &app.session;
    marker_view::placed_with(
        session.markers(),
        |m| Gameplay::of(Some(scenario), m),
        |m| {
            scenario
                .object_tag(m.block, m.index)
                .map_or(game_markers::Item::Equipment, |r| {
                    marker_view::item_of_group(r.group.0)
                })
        },
        |_| false,
        session.volumes(),
        |m| session.marker_area(m),
    )
}

fn row(x: &Placed, mesh: &Mesh) -> Value {
    json!({
        "id": x.id,
        "kind": x.marker.kind.name(),
        "layer": x.layer.id(),
        "team": x.marker.team,
        "position": x.place.at,
        "facing": x.place.forward,
        "label": x.label,
        "number": x.number,
        "area": x.marker.kind.area().map(area_json),
        "area_from": x.area_from,
        "members": x.members.iter()
            .map(|(b, i)| format!("{}[{i}]", marker_view::block_name(*b)))
            .collect::<Vec<_>>(),
        "bounds": bounds_json(mesh.bounds()),
        "lines": mesh.lines.len(),
        "triangles": mesh.tris.len(),
    })
}

fn unknown_kind(k: &str) -> agent_proto::RpcError {
    let mut names: Vec<&str> = Kind::samples().iter().map(Kind::name).collect();
    names.dedup();
    err(
        code::INVALID_PARAMS,
        format!("kind: one of {}, not {k:?}", names.join(", ")),
    )
}

fn size(p: &P) -> Result<[u32; 2], agent_proto::RpcError> {
    let Some(v) = p.get("size") else {
        return Ok([1280, 720]);
    };
    let a: Option<Vec<u32>> = v.as_array().map(|a| {
        a.iter()
            .filter_map(|x| x.as_u64().and_then(|n| u32::try_from(n).ok()))
            .collect()
    });
    match a.as_deref() {
        Some(&[w, h]) if (1..=8192).contains(&w) && (1..=8192).contains(&h) => Ok([w, h]),
        _ => Err(err(
            code::INVALID_PARAMS,
            format!("size: [w, h] of 1..8192, not {v}"),
        )),
    }
}

/// `area` of `markers.render`: a radius or a path, in plain numbers.
fn area_param(v: &Value) -> Result<Area, agent_proto::RpcError> {
    let bad = |why: &str| err(code::INVALID_PARAMS, format!("area: {why}, not {v}"));
    let num = |k: &str, default: f32| match v.get(k) {
        None => Ok(default),
        Some(x) => x
            .as_f64()
            .map(|f| f as f32)
            .ok_or_else(|| bad(&format!("{k} a number"))),
    };
    let (below, above) = (num("below", Area::default().below)?, num("above", Area::default().above)?);
    let mut area = match (v.get("radius"), v.get("path")) {
        (Some(r), None) => Area::radius(
            r.as_f64().ok_or_else(|| bad("radius a number"))? as f32,
        ),
        (None, Some(points)) => {
            let points: Option<Vec<[f32; 2]>> = points.as_array().and_then(|a| {
                a.iter()
                    .map(|p| {
                        let p = p.as_array().filter(|p| p.len() == 2)?;
                        Some([p[0].as_f64()? as f32, p[1].as_f64()? as f32])
                    })
                    .collect()
            });
            Area::path(points.ok_or_else(|| bad("path a list of [x, y]"))?)
        }
        _ => return Err(bad("{radius} or {path} (one of them)")),
    };
    area.below = below;
    area.above = above;
    area.validate()
        .map_err(|e| err(code::INVALID_PARAMS, format!("area: {e}")))?;
    Ok(area)
}

impl Markers {
    fn render(&mut self, p: &P) -> Reply {
        let size = size(p)?;
        let view = p.opt_str("view")?.unwrap_or("angled");
        let (yaw, pitch) = match view {
            "angled" => (-1.9_f32, 0.8_f32),
            "top" => (-std::f32::consts::FRAC_PI_2, 1.5),
            "front" => (-std::f32::consts::FRAC_PI_2, 0.2),
            _ => {
                return Err(err(
                    code::INVALID_PARAMS,
                    format!("view: angled, top or front, not {view:?}"),
                ));
            }
        };
        let team = match p.opt_u64("team")? {
            Some(t) if t < 8 => Some(Some(t as u8)),
            Some(t) => {
                return Err(err(code::INVALID_PARAMS, format!("team: 0 to 7, not {t}")));
            }
            None => None,
        };
        let mut kinds = Kind::samples();
        if let Some(k) = p.opt_str("kind")? {
            kinds.retain(|s| s.name() == k);
            if kinds.is_empty() {
                return Err(unknown_kind(k));
            }
        }
        if let Some(v) = p.get("area") {
            let area = area_param(v)?;
            if kinds.iter().all(|k| k.area().is_none()) {
                return Err(err(
                    code::INVALID_PARAMS,
                    "area: the kind has no area (hill, territory and siege_site do)",
                ));
            }
            for k in &mut kinds {
                match k {
                    Kind::Hill(a) | Kind::Territory(a) | Kind::Site(a) => *a = area.clone(),
                    _ => {}
                }
            }
        }
        let out = match p.opt_str("out")? {
            Some(o) => PathBuf::from(o),
            None => {
                let stem = match p.opt_str("kind")? {
                    Some(k) => k.to_owned(),
                    None => "sheet".to_owned(),
                };
                studio_proto::output_dir()
                    .join("markers")
                    .join(format!("{stem}_{view}.png"))
            }
        };
        super::docs::outside_git(&out)?;

        // The items: each kind in each team it is drawn in.
        let palette = Palette::default();
        let teams: Vec<Option<u8>> = match team {
            Some(t) => vec![t],
            None => vec![Some(0), Some(1), None],
        };
        let mut items: Vec<(Kind, Option<u8>)> = Vec::new();
        for k in kinds {
            let varies = !matches!(k.tint(), Tint::Fixed(_));
            for &t in &teams {
                if varies || items.last().is_none_or(|(l, _)| *l != k) {
                    items.push((k.clone(), t));
                }
            }
        }
        let meshes: Vec<Mesh> = items
            .iter()
            .map(|(k, t)| Marker::new(k.clone(), *t).mesh(&palette))
            .collect();
        // One cell for all, wide enough for the widest.
        let reach = meshes
            .iter()
            .flat_map(|m| m.points())
            .fold(0.0_f32, |r, q| r.max(q[0].abs()).max(q[1].abs()));
        let cell = (2.0 * reach + 0.6).max(2.4);
        let aspect = size[0] as f32 / size[1] as f32;
        let cols = ((items.len() as f32 * aspect).sqrt().ceil() as usize)
            .clamp(1, items.len());
        let rows = items.len().div_ceil(cols);
        let at = |col: usize, row: usize| {
            [
                (col as f32 - (cols as f32 - 1.0) / 2.0) * cell,
                ((rows as f32 - 1.0) / 2.0 - row as f32) * cell,
            ]
        };

        let (mut faces, mut lines) = (Vec::new(), Vec::new());
        let mut shown = Vec::new();
        for (i, ((kind, t), mesh)) in items.iter().zip(&meshes).enumerate() {
            let (col, row) = (i % cols, i / cols);
            let [x, y] = at(col, row);
            // A volume stands on the floor, whatever its solid.
            let lift = match kind {
                Kind::Volume(..) => -mesh.bounds().map_or(0.0, |b| b.0[2]),
                _ => 0.0,
            };
            let placed = mesh.placed(&game_markers::Place::facing([x, y, lift], 0.0));
            let (f, l) = marker_view::vertices(&placed);
            faces.extend(f);
            lines.extend(l);
            shown.push((kind, *t, col, row, [x, y, 0.35], placed.bounds(), mesh));
        }
        // The floor, and its grid a cell apart.
        let half = [cols as f32 * cell / 2.0, rows as f32 * cell / 2.0];
        let floor = [
            [-half[0], -half[1], 0.0],
            [half[0], -half[1], 0.0],
            [half[0], half[1], 0.0],
            [-half[0], half[1], 0.0],
        ];
        let tris = [
            Tri { p: [floor[0], floor[1], floor[2]], n: [0.0, 0.0, 1.0] },
            Tri { p: [floor[0], floor[2], floor[3]], n: [0.0, 0.0, 1.0] },
        ];
        let mut grid = Vec::new();
        let mut rule = |a: [f32; 3], b: [f32; 3]| {
            for position in [a, b] {
                grid.push(render::Vertex {
                    position,
                    center: position,
                    normal: [0.0, 0.0, 1.0],
                    color: [0.28, 0.30, 0.34],
                });
            }
        };
        for c in 0..=cols {
            let x = -half[0] + c as f32 * cell;
            rule([x, -half[1], 0.004], [x, half[1], 0.004]);
        }
        for r in 0..=rows {
            let y = -half[1] + r as f32 * cell;
            rule([-half[0], y, 0.004], [half[0], y, 0.004]);
        }

        let gpu = self
            .gpu
            .get_or_insert_with(|| Gpu::new().map_err(|e| format!("{e:#}")));
        let gpu = match gpu {
            Ok(g) => g,
            Err(e) => return Err(err(code::NO_GPU, format!("no renderer here: {e}"))),
        };
        if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)
                .map_err(|e| err(code::FAILED, format!("{}: {e}", dir.display())))?;
        }
        let mut cam = Camera {
            target: [0.0, 0.0, 0.3],
            yaw,
            pitch,
            ..Camera::default()
        };
        let tan = (cam.fov_y * 0.5).tan();
        let fit = (half[1] / tan).max(half[0] / (tan * aspect));
        cam.distance = (fit * 1.15 + 1.5).max(3.0);
        let mut vp = Viewport::new(&gpu.device);
        vp.set_layers(&Layers::default());
        vp.set_world(&gpu.device, &tris);
        vp.set_layer_overlay(&gpu.device, LayerId::Flags, &faces, &lines);
        vp.set_layer_overlay(&gpu.device, LayerId::Markers, &[], &grid);
        vp.render(&gpu.device, &gpu.queue, &cam, size);
        let texture = vp
            .color_texture()
            .ok_or_else(|| err(code::FAILED, "the viewport drew nothing"))?;
        let rgba = crate::headless::read_texture(&gpu.device, &gpu.queue, texture)
            .map_err(|e| err(code::FAILED, format!("{e:#}")))?;
        crate::headless::write_png(&out, &rgba, texture.width(), texture.height())
            .map_err(|e| err(code::FAILED, format!("{e:#}")))?;
        // What is not the clear colour of the corner pixel.
        let corner: [u8; 3] = [rgba[0], rgba[1], rgba[2]];
        let covered = rgba
            .chunks_exact(4)
            .filter(|px| (0..3).any(|k| px[k].abs_diff(corner[k]) > 2))
            .count();
        let (w, h) = (texture.width() as f32, texture.height() as f32);
        let kinds: Vec<Value> = shown
            .iter()
            .map(|(kind, t, col, row, centre, bounds, mesh)| {
                let px = cam
                    .project(*centre, w / h)
                    .map(|[u, v]| [(u * w).round(), (v * h).round()]);
                json!({
                    "kind": kind.name(),
                    "team": t,
                    "cell": [col, row],
                    "center": px,
                    "lines": mesh.lines.len(),
                    "triangles": mesh.tris.len(),
                    "bounds": bounds_json(*bounds),
                })
            })
            .collect();
        Ok(json!({
            "path": out.display().to_string(),
            "width": texture.width(),
            "height": texture.height(),
            "view": view,
            "covered": covered,
            "kinds": kinds,
        }))
    }
}
