//! `model.*` ([`super::Provider`], registered in [`super::seams`]): a
//! map's objects and their models, read; a model drawn offscreen as the
//! ground sees it, in a biped's hand, or through the first-person camera;
//! and the checks a content author runs before handing a weapon or an
//! object back (docs/editor/agent-api.md, "`model.*`").
//!
//! The provider reads the map the scenario tool has open, or one of the
//! source's maps by name; with `pogo: true`, that map with the Pogo
//! Stick's kit added (`content_kit::inject::add_pogo`, which the `pogo` package's
//! build equals). It holds
//! that map read-only: a fix is a `tag.set` on the map the Tags tool edits
//! (each finding names its path), or, for the kit's own tags, a change to
//! `crates/tools/content-kit`.
//!
//! Renders go through the viewport's native object renderer
//! (`scenario_edit::native_view::preview`), to a PNG under the cache
//! directory's `studio/model/`, never into a Git checkout.

use std::path::PathBuf;
use std::sync::Arc;

use agent_proto::RpcError;
use map::{FourCC, TagId};
use content_kit::Style;
use content_kit::inject::Costs;
use scenario_edit::MapSource as EditSource;
use scenario_edit::geometry::Camera;
use scenario_edit::native_view::preview::{self, Matrix, Piece};
use scenario_edit::save::MapReader;
use serde_json::{Value, json};
use studio_proto::{code, method as m};
use tags::model::RenderModelGeometry;
use tags::object::{Context, ObjectHeader, VariantChoice, is_object_group};

use super::scenario::{group_text, tag_id};
use super::view::Gpu;
use super::{MethodDef, P, Provider, Reply, err};
use crate::App;

/// The `model.*` provider: the map it reads, and a GPU for renders.
#[derive(Default)]
pub struct Models {
    open: Option<Open>,
    gpu: Option<Result<Gpu, String>>,
}

/// The map `model.*` reads.
struct Open {
    name: String,
    source: EditSource,
    /// Read from the scenario tool's map (followed when it changes), not
    /// asked for by name.
    auto: bool,
    pogo: bool,
    kit: Option<Kit>,
    ctx: Context,
    meta_size: u32,
    cache_type: map::CacheType,
}

/// The Pogo Stick's kit in the map read.
struct Kit {
    /// Each look's `weap`.
    weapons: Vec<(Style, TagId)>,
    /// What adding it cost, when Studio added it (not when the map came
    /// with it).
    costs: Option<Costs>,
    from_cache: bool,
}

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

/// Default first-person horizontal field of view, degrees: the game's
/// unzoomed default (docs/game/settings-menu.md, "Field of View").
const FIRST_PERSON_FOV: f32 = 70.0;
/// The renders' background, sRGB-encoded.
const BACKGROUND: [f64; 3] = [0.10, 0.11, 0.13];
/// Below this much free tag memory a check warns.
const TAG_MEMORY_WARN: u32 = 64 << 10;

/// `weap +0x2a8`: the first-person block, 0x10 per player character (the
/// model, `mode`, at `+0`; the animations, `jmad`, at `+8`).
const WEAP_FIRST_PERSON: usize = 0x2a8;
const FIRST_PERSON_SIZE: usize = 0x10;
/// `weap +0x2b0`: the weapon's HUD.
const WEAP_HUD: usize = 0x2b0;
/// `weap +0x2d0`: the barrels, 0xec each, the marker name at `+0x30`.
const WEAP_BARRELS: usize = 0x2d0;
const BARREL_SIZE: usize = 0xec;
const BARREL_MARKER: usize = 0x30;
/// `unit +0x158`, `+0x15c`: the markers a held weapon attaches to.
const UNIT_RIGHT_HAND: usize = 0x158;
/// `matg +0x138`: the player representations, 0xbc each: the
/// first-person hands (`mode`) at `+0`, the unit at `+0xb0`.
const MATG_PLAYERS: usize = 0x138;
const PLAYER_SIZE: usize = 0xbc;
const PLAYER_UNIT: usize = 0xb0;

const MAP_PARAMS: &str = r#""map?":"string (a map of the source; default: the scenario tool's)","pogo?":"boolean (the map with the Pogo Stick's kit added as the game serves it)""#;

fn methods() -> Vec<MethodDef> {
    // The shapes share the map params; leaked once per process (four
    // short strings) so the table stays `&'static`.
    fn with_map(rest: &str) -> &'static str {
        let s = if rest.is_empty() {
            format!("{{{MAP_PARAMS}}}")
        } else {
            format!("{{{rest},{MAP_PARAMS}}}")
        };
        Box::leak(s.into_boxed_str())
    }
    use std::sync::OnceLock;
    static TABLE: OnceLock<Vec<MethodDef>> = OnceLock::new();
    TABLE
        .get_or_init(|| {
            vec![
                def(
                    m::MODEL_LIST,
                    "The objects of the map model.* reads (bipeds by default; `group` picks another object group, or `all`) with their hlmt, render model, variants and region count, and a weapon's first-person entries. Opens the map first: `map` (a map of the source; default the scenario tool's) and `pogo` (with the Pogo Stick's kit added, as the game serves it)",
                    with_map(r#""group?":"string (bipd, weap, ... or all)""#),
                    r#"{"map":"string","pogo":"{weapons:[{style,tag,name}],costs,from_cache}|null","tag_memory":"{used,limit,free}","objects":"[{tag,name,group,model,render_model,variants,regions,first_person?,error?}]"}"#,
                    r#"{"group":"weap","map":"court"}"#,
                ),
                def(
                    m::MODEL_READ,
                    "One object's model: its hlmt and linked tags, the variant drawn and every variant's name, the variant's regions with each permutation, its probability and its tag.set path, the render model's nodes and markers, the change colours, and a weapon's first-person entries",
                    with_map(r#""tag":"string (0x... id, group:name, name, or a unique part of one)","variant?":"string""#),
                    r#"{"tag":"string","name":"string","group":"string","model":"{tag,name}|null","render_model":"string|null","collision_model":"string|null","physics_model":"string|null","animation":"string|null","variant":"string|null","variants":"[string]","regions":"[{region,model_region,permutations:[{name,probability,in_render_model,path}]}]","nodes":"integer","markers":"[{name,node}]","change_colors":"integer","first_person?":"[{index,model,animations,paths}]","paths":"{field: tag.set path}"}"#,
                    r#"{"tag":"pogo_unsc","map":"court"}"#,
                ),
                def(
                    m::MODEL_RENDER,
                    "Draw an object's model offscreen with the viewport's renderer to a PNG (default: the cache directory's studio/model/; never into a Git checkout). `view`: ground (the world model, orbited), third_person (on a player biped, at its right-hand marker) or first_person (the weap's first-person model and the player's hands, from the first-person camera). A view that cannot be drawn is REFUSED with data.reason (no_first_person, first_person_no_model, no_player_biped, hand_marker_missing, ...)",
                    with_map(r#""tag":"string","view?":"ground|third_person|first_person","variant?":"string","character?":"integer (player representation and first-person entry, default 0)","biped?":"string (third_person: the biped, default the character's)","hands?":"boolean (first_person, default true)","size?":"[w,h] (default [640,480])","camera?":{"yaw?":"number","pitch?":"number","distance?":"number","target?":"[x,y,z]","fov?":"number (first_person, degrees across)"},"out?":"string""#),
                    r#"{"path":"string","width":"integer","height":"integer","view":"string","covered":"integer (pixels off the background)","pieces":"[{tag,name,mode,triangles,untextured,picked}]","camera":"object","notes":"[string]"}"#,
                    r#"{"tag":"pogo_unsc","map":"court","view":"ground","size":[64,48],"out":"$OUT/model.png"}"#,
                ),
                def(
                    m::MODEL_CHECK,
                    "Check an object or weapon as content: a missing or empty first-person block, links that do not resolve, markers the game reads that the model lacks, variant permutations without sections, untextured materials, and the map's tag memory left. Each finding: {code, severity (error|warning|note), message, tag, path (its tag.set path, or null)}",
                    with_map(r#""tag":"string","variant?":"string""#),
                    r#"{"tag":"string","name":"string","group":"string","map":"string","editable":"boolean","findings":"[{code,severity,message,tag,path}]","counts":"{error,warning,note}","tag_memory":"{used,limit,free}","pogo":"object|null"}"#,
                    r#"{"tag":"pogo_unsc","map":"court"}"#,
                ),
            ]
        })
        .clone()
}

impl Provider for Models {
    fn methods(&self) -> Vec<MethodDef> {
        methods()
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        if !matches!(
            method,
            m::MODEL_LIST | m::MODEL_READ | m::MODEL_RENDER | m::MODEL_CHECK
        ) {
            return None;
        }
        let p = P(params);
        Some(self.answer(app, method, &p))
    }
}

/// An error with a machine-readable reason in `data.reason`.
fn reason(code: i64, why: &str, message: impl Into<String>) -> RpcError {
    let mut e = err(code, message);
    e.data = Some(json!({ "reason": why }));
    e
}

fn failed(e: impl std::fmt::Display) -> RpcError {
    err(code::FAILED, e.to_string())
}

impl Models {
    fn answer(&mut self, app: &mut App, method: &str, p: &P) -> Reply {
        self.ensure(app, p)?;
        match method {
            m::MODEL_LIST => self.list(p),
            m::MODEL_READ => self.read(p),
            m::MODEL_RENDER => self.render(p),
            m::MODEL_CHECK => self.check(p),
            _ => Err(err(code::METHOD_NOT_FOUND, format!("no method {method}"))),
        }
    }

    /// Opens the map `p` asks for, unless it is the one open. `pogo`
    /// left out keeps the open map's choice.
    fn ensure(&mut self, app: &App, p: &P) -> Result<(), RpcError> {
        let mut pogo = p.bool("pogo")?;
        let (name, source, auto) = match p.opt_str("map")? {
            Some(name) => {
                let name = name.strip_suffix(".map").unwrap_or(name).to_owned();
                if let Some(o) = &self.open
                    && !o.auto
                    && o.name.eq_ignore_ascii_case(&name)
                {
                    if !p.has("pogo") {
                        pogo = o.pogo;
                    }
                    if o.pogo == pogo {
                        return Ok(());
                    }
                }
                let src = app.source().ok_or_else(|| {
                    err(code::NO_DOCUMENT, "no source is open: source.open {\"path\": ...}")
                })?;
                let (path, region) = src
                    .location(&name)
                    .map_err(|e| err(code::INVALID_PARAMS, format!("map: {e:#}")))?;
                (name, EditSource::region(path, region), false)
            }
            None => {
                let scenario = app.loaded_map().zip(app.session.source());
                if let Some(o) = &self.open {
                    let same_scenario = scenario.is_some_and(|(n, _)| n == o.name);
                    if (!o.auto || same_scenario) && (!p.has("pogo") || o.pogo == pogo) {
                        return Ok(());
                    }
                    if !o.auto && p.has("pogo") {
                        // The same named map, with or without the kit.
                        let (name, source) = (o.name.clone(), o.source_base());
                        self.open = Some(open_map(app, name, source, false, pogo)?);
                        return Ok(());
                    }
                }
                let (name, source) = scenario.ok_or_else(|| {
                    err(
                        code::NO_DOCUMENT,
                        "no map is open: model.list {\"map\": ...} (source.maps lists them)",
                    )
                })?;
                (name.to_owned(), source.clone(), true)
            }
        };
        self.open = Some(open_map(app, name, source, auto, pogo)?);
        Ok(())
    }

    fn open(&self) -> &Open {
        self.open.as_ref().expect("ensure opened a map")
    }

    fn list(&self, p: &P) -> Reply {
        let o = self.open();
        let group = p.opt_str("group")?.unwrap_or("bipd");
        let ctx = &o.ctx;
        let mut tags: Vec<_> = ctx
            .catalog
            .tags
            .values()
            .filter(|t| is_object_group(t.group))
            .filter(|t| group == "all" || group_text(t.group) == group)
            .collect();
        tags.sort_by_key(|t| t.id.0);
        let objects: Vec<Value> = tags
            .iter()
            .map(|t| {
                let mut v = json!({
                    "tag": tag_id(t.id),
                    "name": t.name,
                    "group": group_text(t.group),
                });
                match ctx.resolve_object(t.id) {
                    Ok(obj) => {
                        if let Some(model) = &obj.model {
                            v["model"] = json!(model.name);
                            v["render_model"] =
                                json!(model.render_model.as_ref().and_then(|l| l.name.clone()));
                            v["variants"] = json!(variant_names(model));
                            v["regions"] = json!(model.region_count);
                        } else {
                            v["model"] = Value::Null;
                        }
                    }
                    Err(e) => v["error"] = json!(e.to_string()),
                }
                if t.group == FourCC(*b"weap")
                    && let Some(data) = tag_data(ctx, t.id)
                {
                    v["first_person"] = json!(elements(ctx, data, WEAP_FIRST_PERSON, FIRST_PERSON_SIZE).len());
                }
                v
            })
            .collect();
        Ok(json!({
            "map": o.name,
            "pogo": o.kit_json(),
            "tag_memory": o.tag_memory_json(),
            "objects": objects,
        }))
    }

    fn read(&self, p: &P) -> Reply {
        let o = self.open();
        let ctx = &o.ctx;
        let object = find_tag(ctx, p.str("tag")?)?;
        let (definition, _) =
            scenario_edit::object_collision::resolve(ctx, object).map_err(failed)?;
        let tag = ctx.decode(definition).map_err(failed)?;
        let header = ObjectHeader::from_record(&tag.layers[0]).map_err(failed)?;
        let mut out = json!({
            "tag": tag_id(definition),
            "name": tag.name,
            "group": group_text(tag.group),
            "change_colors": header.change_colors,
            "paths": {"model": "model", "default_variant": "default_variant"},
        });
        let resolved = match header.model.id {
            id if id == TagId::NULL => None,
            id => {
                let sid = match p.opt_str("variant")? {
                    Some(v) => preview::string_id(ctx, v).ok_or_else(|| {
                        err(code::INVALID_PARAMS, format!("variant: the map has no {v:?}"))
                    })?,
                    None => header.default_variant,
                };
                Some(ctx.resolve_model(id, sid).map_err(failed)?)
            }
        };
        match &resolved {
            None => out["model"] = Value::Null,
            Some(r) => {
                let linked = |l: &Option<tags::object::Linked>| {
                    json!(l.as_ref().and_then(|l| l.name.clone()))
                };
                out["model"] = json!({ "tag": tag_id(r.id), "name": r.name });
                out["render_model"] = linked(&r.render_model);
                out["collision_model"] = linked(&r.collision_model);
                out["physics_model"] = linked(&r.physics_model);
                out["animation"] = linked(&r.animation);
                out["variant"] = match &r.variant {
                    VariantChoice::First { name, .. } | VariantChoice::Named { name, .. } => {
                        json!(name)
                    }
                    VariantChoice::Missing(_) | VariantChoice::None => Value::Null,
                };
                out["variants"] = json!(variant_names(r));
                let v = r.variant.index();
                out["regions"] = Value::Array(
                    r.regions
                        .iter()
                        .enumerate()
                        .map(|(ri, region)| {
                            json!({
                                "region": region.region,
                                "model_region": region.model_region,
                                "permutations": region.permutations.iter().enumerate().map(|(pi, perm)| json!({
                                    "name": perm.name,
                                    "probability": perm.probability,
                                    "in_render_model": perm.model_permutation.is_some(),
                                    "path": v.map(|v| format!("variants[{v}].regions[{ri}].permutations[{pi}].probability")),
                                })).collect::<Vec<_>>(),
                            })
                        })
                        .collect(),
                );
                out["nodes"] = json!(r.node_count);
                let markers = r
                    .render_model
                    .as_ref()
                    .map(|l| preview::markers(ctx, l.id).unwrap_or_default())
                    .unwrap_or_default();
                out["markers"] = Value::Array(
                    markers
                        .iter()
                        .map(|mk| json!({ "name": mk.name, "node": mk.node }))
                        .collect(),
                );
                out["paths"]["render_model"] = json!("render_model");
            }
        }
        if tag.group == FourCC(*b"weap")
            && let Some(data) = tag_data(ctx, definition)
        {
            let name = |id: Option<TagId>| id.and_then(|id| ctx.catalog.get(id)).and_then(|t| t.name.clone());
            out["first_person"] = Value::Array(
                elements(ctx, data, WEAP_FIRST_PERSON, FIRST_PERSON_SIZE)
                    .iter()
                    .enumerate()
                    .map(|(i, e)| {
                        json!({
                            "index": i,
                            "model": name(tag_ref(e, 0)),
                            "animations": name(tag_ref(e, 8)),
                            "paths": {
                                "model": format!("first_person[{i}].model"),
                                "animations": format!("first_person[{i}].animations"),
                            },
                        })
                    })
                    .collect(),
            );
            out["paths"]["first_person"] = json!("first_person");
        }
        Ok(out)
    }

    fn render(&mut self, p: &P) -> Reply {
        let view = p.opt_str("view")?.unwrap_or("ground");
        if !matches!(view, "ground" | "third_person" | "first_person") {
            return Err(err(
                code::INVALID_PARAMS,
                format!("view: ground, third_person or first_person, not {view:?}"),
            ));
        }
        let size = size(p)?;
        let o = self.open();
        let object = find_tag(&o.ctx, p.str("tag")?)?;
        let stem = o
            .ctx
            .catalog
            .get(object)
            .and_then(|t| t.name.clone())
            .unwrap_or_else(|| tag_id(object))
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or("model")
            .to_owned();
        let out = match p.opt_str("out")? {
            Some(o) => PathBuf::from(o),
            None => studio_proto::output_dir()
                .join("model")
                .join(format!("{stem}_{view}.png")),
        };
        super::docs::outside_git(&out)?;
        // Everything the picture needs, read before any GPU is asked for:
        // a view that cannot be drawn says why on any machine.
        let scene = o.scene(p, object, view, size)?;
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
        let placed: Vec<(&Piece, _)> = scene.pieces.iter().map(|(pc, at)| (pc, *at)).collect();
        let texture = preview::render(
            &gpu.device,
            &gpu.queue,
            &placed,
            &scene.camera,
            size,
            BACKGROUND,
        );
        let rgba = crate::headless::read_texture(&gpu.device, &gpu.queue, &texture)
            .map_err(|e| err(code::FAILED, format!("{e:#}")))?;
        crate::headless::write_png(&out, &rgba, texture.width(), texture.height())
            .map_err(|e| err(code::FAILED, format!("{e:#}")))?;
        let covered = preview::covered(&rgba, BACKGROUND);
        let mut notes = scene.notes;
        if covered == 0 {
            notes.push(match view {
                "first_person" => "nothing is in the first-person camera's view: the first-person models in their rest pose sit outside it (the game poses them with the first-person animations)".to_owned(),
                _ => "the picture is empty: the camera sees none of the model".to_owned(),
            });
        }
        let ctx = &self.open().ctx;
        Ok(json!({
            "path": out.display().to_string(),
            "width": texture.width(),
            "height": texture.height(),
            "view": view,
            "covered": covered,
            "pieces": scene.pieces.iter().map(|(pc, _)| piece_json(ctx, pc)).collect::<Vec<_>>(),
            "camera": camera_json(&scene.camera),
            "notes": notes,
        }))
    }

    fn check(&self, p: &P) -> Reply {
        let o = self.open();
        let ctx = &o.ctx;
        let object = find_tag(ctx, p.str("tag")?)?;
        let (definition, _) =
            scenario_edit::object_collision::resolve(ctx, object).map_err(failed)?;
        let t = ctx
            .catalog
            .get(definition)
            .ok_or_else(|| err(code::INVALID_PARAMS, "tag: no such tag"))?;
        let mut map = o.source.open().map_err(failed)?;
        let mut files = scenario_edit::scene::resource_files(&o.source);
        let findings = checks::run(
            ctx,
            &mut map,
            &mut files,
            definition,
            p.opt_str("variant")?,
        );
        let mut findings = findings;
        findings.push(o.tag_memory_finding());
        let count = |s: &str| findings.iter().filter(|f| f.severity == s).count();
        Ok(json!({
            "tag": tag_id(definition),
            "name": t.name,
            "group": group_text(t.group),
            "map": o.name,
            "editable": o.kit.as_ref().is_none_or(|k| k.costs.is_none()),
            "counts": {"error": count("error"), "warning": count("warning"), "note": count("note")},
            "findings": findings.iter().map(checks::Finding::json).collect::<Vec<_>>(),
            "tag_memory": o.tag_memory_json(),
            "pogo": o.kit_json(),
        }))
    }
}

/// What a render draws: the pieces placed, the camera, and notes.
struct Scene {
    pieces: Vec<(Piece, Matrix)>,
    camera: Camera,
    notes: Vec<String>,
}

impl Open {
    /// The map the kit went into (or the map itself).
    fn source_base(&self) -> EditSource {
        if self.kit.as_ref().is_some_and(|k| k.costs.is_some()) {
            EditSource {
                bytes: None,
                ..self.source.clone()
            }
        } else {
            self.source.clone()
        }
    }

    fn kit_json(&self) -> Value {
        let Some(k) = &self.kit else {
            return Value::Null;
        };
        json!({
            "weapons": k.weapons.iter().map(|(style, id)| json!({
                "style": style.id(),
                "tag": tag_id(*id),
                "name": style.tag_path(),
            })).collect::<Vec<_>>(),
            "costs": k.costs.map(|c| json!({
                "tags": c.tags,
                "tag_growth": c.tag_growth,
                "available": c.available,
                "file_growth": c.file_growth,
            })),
            "from_cache": k.from_cache,
        })
    }

    fn tag_memory(&self) -> Option<(u32, u32)> {
        let limit = map::writer::max_metadata_size(self.cache_type).ok()?;
        Some((self.meta_size, limit))
    }

    fn tag_memory_json(&self) -> Value {
        match self.tag_memory() {
            Some((used, limit)) => json!({
                "used": used,
                "limit": limit,
                "free": i64::from(limit) - i64::from(used),
            }),
            None => Value::Null,
        }
    }

    fn tag_memory_finding(&self) -> checks::Finding {
        let after = if self.kit.as_ref().is_some_and(|k| k.costs.is_some()) {
            " with the Pogo Stick's kit in it"
        } else {
            ""
        };
        match self.tag_memory() {
            None => checks::Finding::new(
                "tag_memory",
                "warning",
                format!("{}: a map of a cache type with no known tag memory limit", self.name),
            ),
            Some((used, limit)) if used > limit => checks::Finding::new(
                "tag_memory",
                "error",
                format!(
                    "{}{after}: tag memory {used} B is over the map type's {limit} B by {} B",
                    self.name,
                    used - limit
                ),
            ),
            Some((used, limit)) => checks::Finding::new(
                "tag_memory",
                if limit - used < TAG_MEMORY_WARN { "warning" } else { "note" },
                format!(
                    "{}{after}: tag memory {used} B of {limit} B, {} B free",
                    self.name,
                    limit - used
                ),
            ),
        }
    }

    /// The pieces, camera and notes of `view` for `object`.
    fn scene(&self, p: &P, object: TagId, view: &str, size: [u32; 2]) -> Result<Scene, RpcError> {
        let ctx = &self.ctx;
        let mut map = self.source.open().map_err(failed)?;
        let mut files = scenario_edit::scene::resource_files(&self.source);
        let variant = p.opt_str("variant")?;
        let character = p.opt_usize("character")?.unwrap_or(0);
        let identity = Matrix::IDENTITY;
        let mut notes = Vec::new();
        let cam = p.get("camera").map(P);
        let pieces = match view {
            "ground" => {
                let piece = preview::object(&mut map, ctx, &mut files, object, variant)
                    .map_err(|e| reason(code::REFUSED, "model_fails", e))?;
                vec![(piece, identity)]
            }
            "third_person" => {
                let biped = match p.opt_str("biped")? {
                    Some(b) => find_tag(ctx, b)?,
                    None => player_unit(ctx, character).ok_or_else(|| {
                        reason(
                            code::REFUSED,
                            "no_player_biped",
                            format!("the map's globals name no player unit for character {character}: give biped"),
                        )
                    })?,
                };
                let body = preview::object(&mut map, ctx, &mut files, biped, None)
                    .map_err(|e| reason(code::REFUSED, "biped_fails", format!("the biped: {e}")))?;
                let weapon = preview::object(&mut map, ctx, &mut files, object, variant)
                    .map_err(|e| reason(code::REFUSED, "model_fails", e))?;
                let (unit, _) =
                    scenario_edit::object_collision::resolve(ctx, biped).map_err(failed)?;
                let hand = tag_data(ctx, unit)
                    .and_then(|d| string_at(ctx, d, UNIT_RIGHT_HAND))
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        reason(
                            code::REFUSED,
                            "hand_marker_missing",
                            "the biped names no right-hand marker (unit right_hand_node)",
                        )
                    })?;
                let markers = preview::markers(ctx, body.mode).map_err(failed)?;
                let marker = markers.iter().find(|mk| mk.name == hand).ok_or_else(|| {
                    reason(
                        code::REFUSED,
                        "hand_marker_missing",
                        format!(
                            "the biped's render model has no marker {hand:?} (unit right_hand_node; it has {})",
                            markers.iter().map(|mk| mk.name.as_str()).collect::<Vec<_>>().join(", ")
                        ),
                    )
                })?;
                let at = preview::marker_frame(&body, marker);
                notes.push(format!(
                    "rest pose: the weapon sits at the biped's {hand:?} marker as the attach puts it, the biped unanimated"
                ));
                vec![(body, identity), (weapon, at)]
            }
            _ => {
                let (weap, _) =
                    scenario_edit::object_collision::resolve(ctx, object).map_err(failed)?;
                let t = ctx.catalog.get(weap).ok_or_else(|| err(code::INVALID_PARAMS, "no such tag"))?;
                if t.group != FourCC(*b"weap") {
                    return Err(reason(
                        code::REFUSED,
                        "not_a_weapon",
                        format!("{} is a {}, and only a weap has a first-person model", t.name.as_deref().unwrap_or("?"), group_text(t.group)),
                    ));
                }
                let data = tag_data(ctx, weap).ok_or_else(|| failed("the weap has no tag data"))?;
                let entries = elements(ctx, data, WEAP_FIRST_PERSON, FIRST_PERSON_SIZE);
                if entries.is_empty() {
                    let mut e = reason(
                        code::REFUSED,
                        "no_first_person",
                        "weap has no first_person entry: the first-person view has no weapon model to draw (weap first_person, one entry per player character)",
                    );
                    e.data.as_mut().expect("reason sets data")["path"] = json!("first_person");
                    return Err(e);
                }
                let entry = entries.get(character).ok_or_else(|| {
                    reason(
                        code::REFUSED,
                        "no_first_person",
                        format!("weap has {} first_person entries, none for character {character}", entries.len()),
                    )
                })?;
                let Some(mode) = tag_ref(entry, 0) else {
                    let mut e = reason(
                        code::REFUSED,
                        "first_person_no_model",
                        format!("weap first_person[{character}].model is null"),
                    );
                    e.data.as_mut().expect("reason sets data")["path"] =
                        json!(format!("first_person[{character}].model"));
                    return Err(e);
                };
                let gun = preview::render_model(&mut map, &mut files, mode).map_err(|e| {
                    reason(code::REFUSED, "first_person_model_fails", format!("the first-person model: {e}"))
                })?;
                let mut pieces = vec![(gun, identity)];
                if p.get("hands").and_then(Value::as_bool).unwrap_or(true) {
                    match player_hands(ctx, character) {
                        Some(hands) => match preview::render_model(&mut map, &mut files, hands) {
                            Ok(h) => pieces.push((h, identity)),
                            Err(e) => notes.push(format!("the player's first-person hands are not drawn: {e}")),
                        },
                        None => notes.push(format!(
                            "the map's globals name no first-person hands for character {character}"
                        )),
                    }
                }
                notes.push(
                    "rest pose: the first-person models unanimated, their origin at the eye, +x ahead".to_owned(),
                );
                pieces
            }
        };
        let camera = if view == "first_person" {
            let fov = match cam.as_ref() {
                Some(c) => c.opt_f32("fov")?.unwrap_or(FIRST_PERSON_FOV),
                None => FIRST_PERSON_FOV,
            };
            if !(1.0..=179.0).contains(&fov) {
                return Err(err(code::INVALID_PARAMS, "camera.fov: 1 to 179 degrees"));
            }
            preview::first_person_camera(fov.to_radians(), size)
        } else {
            let placed: Vec<(&Piece, _)> = pieces.iter().map(|(pc, at)| (pc, *at)).collect();
            let bounds = preview::placed_bounds(&placed).unwrap_or(([-0.5; 3], [0.5; 3]));
            let yaw = if view == "third_person" { 0.5 } else { 0.8 };
            let mut c = preview::framing(bounds, yaw, 0.35);
            if let Some(q) = &cam {
                orbit(&mut c, q)?;
            }
            c
        };
        Ok(Scene {
            pieces,
            camera,
            notes,
        })
    }
}

/// Opens map `name` from `source`, with the kit added when `pogo`.
fn open_map(_app: &App, name: String, source: EditSource, auto: bool, pogo: bool) -> Result<Open, RpcError> {
    let mut source = source;
    let mut kit = None;
    let marker = content_kit::inject::marker_string();
    if pogo {
        let original = source.open().map_err(failed)?;
        let has_kit = original
            .strings()
            .iter()
            .flatten()
            .any(|s| s.starts_with("pogo_kit_v"));
        drop(original);
        if has_kit {
            kit = Some(Kit {
                weapons: Vec::new(),
                costs: None,
                from_cache: false,
            });
        } else {
            let bytes = match &source.bytes {
                Some(b) => b.to_vec(),
                None => source.read_base().map_err(failed)?,
            };
            let stem = name.to_ascii_lowercase();
            let added = content_kit::inject::add_pogo(&bytes, &stem).map_err(|e| {
                reason(
                    code::REFUSED,
                    "pogo_refused",
                    format!("{name}: the Pogo Stick's kit does not go in: {e:#}"),
                )
            })?;
            kit = Some(Kit {
                weapons: Vec::new(),
                costs: Some(added.costs),
                from_cache: false,
            });
            source = EditSource {
                bytes: Some(Arc::from(added.bytes)),
                ..source
            };
        }
    }
    let mut map = source.open().map_err(failed)?;
    let ctx = Context::read(&mut map).map_err(failed)?;
    let header = map.header().clone();
    if let Some(k) = &mut kit {
        k.weapons = Style::ALL
            .into_iter()
            .filter_map(|style| {
                let path = style.tag_path();
                ctx.catalog
                    .tags
                    .values()
                    .find(|t| t.group == FourCC(*b"weap") && t.name.as_deref() == Some(path.as_str()))
                    .map(|t| (style, t.id))
            })
            .collect();
    } else if map.strings().iter().flatten().any(|s| *s == marker) {
        // The map came with the kit: name its weapons too.
        kit = Some(Kit {
            weapons: Style::ALL
                .into_iter()
                .filter_map(|style| {
                    let path = style.tag_path();
                    ctx.catalog
                        .tags
                        .values()
                        .find(|t| t.group == FourCC(*b"weap") && t.name.as_deref() == Some(path.as_str()))
                        .map(|t| (style, t.id))
                })
                .collect(),
            costs: None,
            from_cache: false,
        });
    }
    Ok(Open {
        name,
        source,
        auto,
        pogo,
        kit,
        ctx,
        meta_size: header.meta_size,
        cache_type: header.cache_type,
    })
}

/// `size`: `[w, h]` of 1 to 4096, default 640 by 480.
pub(super) fn size(p: &P) -> Result<[u32; 2], RpcError> {
    let Some(v) = p.get("size") else {
        return Ok([640, 480]);
    };
    let a: Option<Vec<u32>> = v.as_array().map(|a| {
        a.iter()
            .filter_map(|x| x.as_u64().and_then(|n| u32::try_from(n).ok()))
            .collect()
    });
    match a.as_deref() {
        Some(&[w, h]) if (1..=4096).contains(&w) && (1..=4096).contains(&h) => Ok([w, h]),
        _ => Err(err(code::INVALID_PARAMS, format!("size: [w, h] of 1..4096, not {v}"))),
    }
}

/// Applies `yaw`, `pitch` (degrees), `distance` and `target` over `c`.
fn orbit(c: &mut Camera, q: &P) -> Result<(), RpcError> {
    if let Some(v) = q.opt_f32("yaw")? {
        c.yaw = v.to_radians();
    }
    if let Some(v) = q.opt_f32("pitch")? {
        c.pitch = v.to_radians().clamp(-1.55, 1.55);
    }
    if let Some(v) = q.opt_f32("distance")? {
        if v <= 0.0 {
            return Err(err(code::INVALID_PARAMS, "camera.distance: more than 0"));
        }
        c.distance = v;
    }
    if let Some(v) = q.get("target") {
        c.target = super::params::vec3("camera.target", v)?;
    }
    Ok(())
}

fn camera_json(c: &Camera) -> Value {
    match &c.exact {
        Some(e) => json!({
            "eye": e.eye,
            "forward": e.forward,
            "up": e.up,
            "fov_y": c.fov_y.to_degrees(),
        }),
        None => json!({
            "yaw": c.yaw.to_degrees(),
            "pitch": c.pitch.to_degrees(),
            "distance": c.distance,
            "target": c.target,
            "fov_y": c.fov_y.to_degrees(),
        }),
    }
}

fn piece_json(ctx: &Context, pc: &Piece) -> Value {
    let name = |id: TagId| ctx.catalog.get(id).and_then(|t| t.name.clone());
    json!({
        "tag": tag_id(pc.model.object),
        "name": name(pc.model.object),
        "mode": name(pc.mode),
        "triangles": pc.triangles(),
        "untextured": pc.untextured,
        "picked": pc.picked.iter().map(|(r, p)| json!({"region": r, "permutation": p})).collect::<Vec<_>>(),
    })
}

fn variant_names(r: &tags::object::ResolvedModel) -> Vec<Option<String>> {
    r.tag
        .layers
        .first()
        .map(|root| {
            root.elements("variants")
                .iter()
                .map(|v| v.sid("name").and_then(|s| s.1.map(str::to_owned)))
                .collect()
        })
        .unwrap_or_default()
}

/// A tag by `0x...` id, `group:name`, its name, or a unique part of an
/// object tag's name (case aside); a lone group names its one object tag.
fn find_tag(ctx: &Context, text: &str) -> Result<TagId, RpcError> {
    let mut all: Vec<_> = ctx.catalog.tags.values().collect();
    all.sort_by_key(|t| t.id.0);
    let none = || err(code::INVALID_PARAMS, format!("tag: no tag {text:?} (model.list lists them)"));
    if let Some(hex) = text.strip_prefix("0x") {
        let n = u32::from_str_radix(hex, 16).map_err(|_| none())?;
        return ctx.catalog.get(TagId(n)).map(|t| t.id).ok_or_else(none);
    }
    let one = |hits: Vec<&tags::object::CatalogTag>| -> Option<Result<TagId, RpcError>> {
        match hits.as_slice() {
            [] => None,
            [t] => Some(Ok(t.id)),
            many => Some(Err(err(
                code::INVALID_PARAMS,
                format!(
                    "tag: {text:?} names {} tags ({}): give group:name or the id",
                    many.len(),
                    many.iter()
                        .take(8)
                        .map(|t| format!("{}:{}", group_text(t.group), t.name.as_deref().unwrap_or("?")))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ))),
        }
    };
    if let Some((g, n)) = text.split_once(':')
        && g.len() <= 4
    {
        let hits = all
            .iter()
            .copied()
            .filter(|t| group_text(t.group) == g && t.name.as_deref() == Some(n))
            .collect();
        return one(hits).unwrap_or_else(|| Err(none()));
    }
    let objects = || all.iter().copied().filter(|t| is_object_group(t.group));
    if text.len() <= 4
        && let Some(r) = one(objects().filter(|t| group_text(t.group) == text).collect())
    {
        return r;
    }
    let exact: Vec<_> = all.iter().copied().filter(|t| t.name.as_deref() == Some(text)).collect();
    let exact_objects: Vec<_> = exact.iter().copied().filter(|t| is_object_group(t.group)).collect();
    if let Some(r) = one(exact_objects) {
        return r;
    }
    if let Some(r) = one(exact) {
        return r;
    }
    let lower = text.to_ascii_lowercase();
    one(objects()
        .filter(|t| t.name.as_deref().is_some_and(|n| n.to_ascii_lowercase().contains(&lower)))
        .collect())
    .unwrap_or_else(|| Err(none()))
}

/// A tag's root bytes.
fn tag_data(ctx: &Context, id: TagId) -> Option<&[u8]> {
    let (address, size) = ctx.catalog.get(id)?.location?;
    ctx.meta.slice(address, u64::from(size)).ok()
}

fn word(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
}

/// The tag a reference at `b[at..]` names, `None` for a null one.
fn tag_ref(b: &[u8], at: usize) -> Option<TagId> {
    word(b, at + 4).map(TagId).filter(|id| *id != TagId::NULL && id.0 != 0)
}

/// The elements of the block at `b[at..]`, each `stride` bytes; none when
/// the block is empty or out of the metadata.
fn elements<'a>(ctx: &'a Context, b: &[u8], at: usize, stride: usize) -> Vec<&'a [u8]> {
    let (Some(count), Some(address)) = (word(b, at), word(b, at + 4)) else {
        return Vec::new();
    };
    if count == 0 || count > 0x10000 {
        return Vec::new();
    }
    match ctx.meta.slice(address, count as u64 * stride as u64) {
        Ok(bytes) => bytes.chunks_exact(stride).collect(),
        Err(_) => Vec::new(),
    }
}

/// The string a string ID at `b[at..]` names.
fn string_at(ctx: &Context, b: &[u8], at: usize) -> Option<String> {
    tags::StringId(word(b, at)?)
        .resolve(&ctx.catalog.strings)
        .ok()
        .flatten()
        .map(str::to_owned)
}

/// The globals' player representation `character`.
fn player(ctx: &Context, character: usize) -> Option<&[u8]> {
    let globals = ctx
        .catalog
        .tags
        .values()
        .find(|t| t.group == FourCC(*b"matg"))?
        .id;
    let data = tag_data(ctx, globals)?;
    elements(ctx, data, MATG_PLAYERS, PLAYER_SIZE)
        .get(character)
        .copied()
}

/// The unit player representation `character` spawns as.
fn player_unit(ctx: &Context, character: usize) -> Option<TagId> {
    tag_ref(player(ctx, character)?, PLAYER_UNIT)
}

/// The first-person hands of player representation `character`.
fn player_hands(ctx: &Context, character: usize) -> Option<TagId> {
    tag_ref(player(ctx, character)?, 0)
}

/// How many player representations the globals have.
fn player_count(ctx: &Context) -> usize {
    (0..)
        .take_while(|&i| player(ctx, i).is_some())
        .count()
}

/// The groups `check.run`'s models row checks when no `target` is given:
/// what a player holds, rides or is.
const CHECKED_GROUPS: [&[u8; 4]; 3] = [b"bipd", b"vehi", b"weap"];

/// `check.run`'s models row ([`super::checks::CHECKS`]): `model.check`'s
/// findings for every biped, vehicle and weapon of the scenario tool's map
/// (or the source's only map), or for the one `target` names, and the
/// map's tag-memory headroom once. Each code is `model.<code>`; the
/// tag.set path that fixes it is in the message (the row's `path` is a
/// checkout file, which a map tag is not). The map is read as it is,
/// without the Pogo kit: `model.check {pogo: true}` checks the kit.
pub fn check(app: &mut App, target: Option<&str>) -> Result<Vec<super::checks::Finding>, String> {
    use super::checks::{Finding as Row, Severity};
    let scenario = app
        .loaded_map()
        .zip(app.session.source())
        .map(|(n, s)| (n.to_owned(), s.clone()));
    let (name, source, auto) = match scenario {
        Some((name, source)) => (name, source, true),
        None => match (app.source(), app.maps()) {
            (Some(src), [only]) => {
                let (path, region) = src.location(only).map_err(|e| format!("{only}: {e:#}"))?;
                (only.clone(), EditSource::region(path, region), false)
            }
            (Some(_), _) => {
                return Err("no scenario is open, and the source has more than one map: open one, or model.check {map} names it".into());
            }
            (None, _) => return Err("no map is open".into()),
        },
    };
    let o = open_map(app, name, source, auto, false).map_err(|e| e.to_string())?;
    let ctx = &o.ctx;
    let objects: Vec<TagId> = match target {
        Some(t) => {
            let object = find_tag(ctx, t).map_err(|e| e.to_string())?;
            vec![scenario_edit::object_collision::resolve(ctx, object).map_err(|e| e.to_string())?.0]
        }
        None => {
            let mut all: Vec<_> = ctx
                .catalog
                .tags
                .values()
                .filter(|t| CHECKED_GROUPS.iter().any(|g| t.group == FourCC(**g)))
                .map(|t| t.id)
                .collect();
            all.sort_by_key(|id| id.0);
            all
        }
    };
    let mut map = o.source.open().map_err(|e| e.to_string())?;
    let mut files = scenario_edit::scene::resource_files(&o.source);
    let mut found = Vec::new();
    for object in objects {
        found.extend(checks::run(ctx, &mut map, &mut files, object, None));
    }
    found.push(o.tag_memory_finding());
    Ok(found
        .into_iter()
        .map(|f| Row {
            area: "models",
            code: format!("model.{}", f.code),
            severity: match f.severity {
                "error" => Severity::Error,
                "warning" => Severity::Warning,
                _ => Severity::Note,
            },
            message: match &f.path {
                Some(p) => format!("{} (tag.set path {p})", f.message),
                None => f.message,
            },
            target: f.tag.unwrap_or_else(|| o.name.clone()),
            path: None,
        })
        .collect())
}

/// `model.check`'s findings.
mod checks {
    use super::*;

    /// One finding: what, how bad, and where to fix it.
    pub struct Finding {
        pub code: &'static str,
        pub severity: &'static str,
        pub message: String,
        /// The tag to edit, as `group:name`.
        pub tag: Option<String>,
        /// Its tag.set path, when one field is the fix.
        pub path: Option<String>,
    }

    impl Finding {
        pub fn new(code: &'static str, severity: &'static str, message: String) -> Self {
            Self {
                code,
                severity,
                message,
                tag: None,
                path: None,
            }
        }

        fn at(mut self, ctx: &Context, tag: TagId, path: impl Into<String>) -> Self {
            self.tag = Some(tag_text(ctx, tag));
            self.path = Some(path.into());
            self
        }

        fn on(mut self, ctx: &Context, tag: TagId) -> Self {
            self.tag = Some(tag_text(ctx, tag));
            self
        }

        pub fn json(&self) -> Value {
            json!({
                "code": self.code,
                "severity": self.severity,
                "message": self.message,
                "tag": self.tag,
                "path": self.path,
            })
        }
    }

    fn tag_text(ctx: &Context, id: TagId) -> String {
        match ctx.catalog.get(id) {
            Some(t) => format!(
                "{}:{}",
                tags::defs::fourcc_text(t.group),
                t.name.as_deref().unwrap_or("")
            ),
            None => tag_id(id),
        }
    }

    fn name(ctx: &Context, id: TagId) -> String {
        ctx.catalog
            .get(id)
            .and_then(|t| t.name.clone())
            .unwrap_or_else(|| tag_id(id))
    }

    /// Every finding for object `object` (a definition, not a collection).
    pub fn run(
        ctx: &Context,
        map: &mut map::Map<MapReader>,
        files: &mut assets::ResourceFiles,
        object: TagId,
        variant: Option<&str>,
    ) -> Vec<Finding> {
        let mut out = Vec::new();
        let tag = match ctx.decode(object) {
            Ok(t) => t,
            Err(e) => {
                out.push(
                    Finding::new("unreadable", "error", format!("{}: {e}", name(ctx, object))).on(ctx, object),
                );
                return out;
            }
        };
        // Every reference the object's layers name must resolve.
        for layer in &tag.layers {
            for (f, v) in &layer.fields {
                if let tags::object::Value::TagRef(r) = v
                    && r.id != TagId::NULL
                    && ctx.catalog.get(r.id).is_none()
                {
                    let path = if f.name == "model" { Some("model") } else { None };
                    let mut finding = Finding::new(
                        "unresolved_link",
                        "error",
                        format!(
                            "{} {} (+{:#x}) names {}, which the map does not have",
                            layer.layout,
                            f.name,
                            f.offset,
                            tag_id(r.id)
                        ),
                    )
                    .on(ctx, object);
                    finding.path = path.map(str::to_owned);
                    out.push(finding);
                }
            }
        }
        let header = match ObjectHeader::from_record(&tag.layers[0]) {
            Ok(h) => h,
            Err(e) => {
                out.push(Finding::new("unreadable", "error", format!("the object header: {e}")).on(ctx, object));
                return out;
            }
        };
        let mut render_markers = Vec::new();
        if header.model.id == TagId::NULL {
            out.push(
                Finding::new(
                    "no_model",
                    "error",
                    format!("{} names no model: nothing is drawn for it", name(ctx, object)),
                )
                .at(ctx, object, "model"),
            );
        } else if ctx.catalog.get(header.model.id).is_some() {
            model_findings(ctx, map, files, object, &header, variant, &mut out, &mut render_markers);
        }
        if tag.group == FourCC(*b"weap") {
            weapon_findings(ctx, map, files, object, &render_markers, &mut out);
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn model_findings(
        ctx: &Context,
        map: &mut map::Map<MapReader>,
        files: &mut assets::ResourceFiles,
        object: TagId,
        header: &ObjectHeader,
        variant: Option<&str>,
        out: &mut Vec<Finding>,
        markers: &mut Vec<String>,
    ) {
        let hlmt = header.model.id;
        let sid = match variant {
            Some(v) => match preview::string_id(ctx, v) {
                Some(s) => s,
                None => {
                    out.push(
                        Finding::new("variant_missing", "error", format!("the map has no string {v:?}: no variant of that name"))
                            .on(ctx, hlmt),
                    );
                    return;
                }
            },
            None => header.default_variant,
        };
        let resolved = match ctx.resolve_model(hlmt, sid) {
            Ok(r) => r,
            Err(e) => {
                out.push(
                    Finding::new("model_invalid", "error", format!("{}: {e}", name(ctx, hlmt))).on(ctx, hlmt),
                );
                return;
            }
        };
        if let VariantChoice::Missing(_) = resolved.variant {
            let path = if variant.is_some() { None } else { Some("default_variant") };
            let mut f = Finding::new(
                "variant_missing",
                "error",
                format!(
                    "{} has no variant {:?}: the game finds none and draws the object without regions",
                    name(ctx, hlmt),
                    variant.unwrap_or("(the default_variant)")
                ),
            )
            .on(ctx, object);
            f.path = path.map(str::to_owned);
            out.push(f);
        }
        let root = &resolved.tag.layers[0];
        for (field, linked) in [
            ("render_model", &resolved.render_model),
            ("collision_model", &resolved.collision_model),
            ("physics_model", &resolved.physics_model),
            ("animation", &resolved.animation),
        ] {
            let named = root.tag_ref(field).map(|r| r.id).filter(|id| *id != TagId::NULL);
            if named.is_some() && linked.is_none() {
                out.push(
                    Finding::new(
                        "unresolved_link",
                        if field == "render_model" { "error" } else { "warning" },
                        format!("{} {field} names {}, which the map does not have", name(ctx, hlmt), tag_id(named.unwrap_or(TagId::NULL))),
                    )
                    .at(ctx, hlmt, field),
                );
            }
        }
        let Some(mode) = resolved.render_model.as_ref().map(|l| l.id) else {
            if root.tag_ref("render_model").is_none_or(|r| r.id == TagId::NULL) {
                out.push(
                    Finding::new("no_render_model", "error", format!("{} names no render model: nothing is drawn", name(ctx, hlmt)))
                        .at(ctx, hlmt, "render_model"),
                );
            }
            return;
        };
        match preview::markers(ctx, mode) {
            Ok(m) => markers.extend(m.into_iter().map(|mk| mk.name)),
            Err(e) => out.push(Finding::new("unreadable", "warning", format!("{} markers: {e}", name(ctx, mode))).on(ctx, mode)),
        }
        let geo = match RenderModelGeometry::read(map, mode, &mut Raw(&mut *files)) {
            Ok(g) => g,
            Err(e) => {
                out.push(
                    Finding::new("render_model_unreadable", "error", format!("{}: {e}", name(ctx, mode))).on(ctx, mode),
                );
                return;
            }
        };
        if resolved.node_count as usize != geo.nodes.len() {
            out.push(
                Finding::new(
                    "node_count_mismatch",
                    "error",
                    format!(
                        "{} has {} nodes, its render model {} has {}: the game binds object node i to render node i",
                        name(ctx, hlmt),
                        resolved.node_count,
                        name(ctx, mode),
                        geo.nodes.len()
                    ),
                )
                .on(ctx, hlmt),
            );
        }
        // Each permutation the variant may pick must have a section.
        let v = resolved.variant.index();
        for (ri, region) in resolved.regions.iter().enumerate() {
            let model_region = geo
                .regions
                .iter()
                .find(|r| r.name.is_some() && r.name == region.region);
            for (pi, perm) in region.permutations.iter().enumerate() {
                let section = model_region
                    .and_then(|r| r.permutations.iter().find(|p| p.name.is_some() && p.name == perm.name))
                    .and_then(|p| p.best_section());
                if section.is_none() {
                    let path = v.map(|v| format!("variants[{v}].regions[{ri}].permutations[{pi}].permutation"));
                    let mut f = Finding::new(
                        "no_sections",
                        "error",
                        format!(
                            "variant region {} permutation {} has no section in {}: the game draws nothing for that region",
                            region.region.as_deref().unwrap_or("?"),
                            perm.name.as_deref().unwrap_or("?"),
                            name(ctx, mode)
                        ),
                    )
                    .on(ctx, hlmt);
                    f.path = path;
                    out.push(f);
                }
            }
        }
        for r in &geo.regions {
            if r.permutations.iter().all(|p| p.best_section().is_none()) {
                out.push(
                    Finding::new(
                        "no_sections",
                        "warning",
                        format!("{} region {} has no permutation with a section", name(ctx, mode), r.name.as_deref().unwrap_or("?")),
                    )
                    .on(ctx, mode),
                );
            }
        }
        match preview::object(map, ctx, files, object, variant) {
            Ok(piece) => {
                if !piece.untextured.is_empty() {
                    out.push(
                        Finding::new(
                            "untextured",
                            "warning",
                            format!(
                                "{} materials {:?} have no base map the renderer decodes: drawn grey in previews",
                                name(ctx, mode),
                                piece.untextured
                            ),
                        )
                        .on(ctx, mode),
                    );
                }
            }
            Err(e) => out.push(Finding::new("model_fails", "error", format!("the model does not load: {e}")).on(ctx, object)),
        }
    }

    fn weapon_findings(
        ctx: &Context,
        map: &mut map::Map<MapReader>,
        files: &mut assets::ResourceFiles,
        weap: TagId,
        markers: &[String],
        out: &mut Vec<Finding>,
    ) {
        let Some(data) = tag_data(ctx, weap) else {
            return;
        };
        let entries = elements(ctx, data, WEAP_FIRST_PERSON, FIRST_PERSON_SIZE);
        let players = player_count(ctx);
        if entries.is_empty() {
            out.push(
                Finding::new(
                    "no_first_person",
                    "error",
                    "weap has no first_person entry: the first-person view has no weapon model (one entry per player character, the model a mode and the animations a jmad)".to_owned(),
                )
                .at(ctx, weap, "first_person"),
            );
        } else if entries.len() < players {
            out.push(
                Finding::new(
                    "first_person_missing_character",
                    "warning",
                    format!(
                        "weap has {} first_person entries for the globals' {players} player characters",
                        entries.len()
                    ),
                )
                .at(ctx, weap, "first_person"),
            );
        }
        for (i, e) in entries.iter().enumerate() {
            let model_path = format!("first_person[{i}].model");
            match tag_ref(e, 0) {
                None => out.push(
                    Finding::new("first_person_no_model", "error", format!("{model_path} is null: the first-person view draws no weapon"))
                        .at(ctx, weap, model_path),
                ),
                Some(id) => match ctx.catalog.get(id) {
                    None => out.push(
                        Finding::new("unresolved_link", "error", format!("{model_path} names {}, which the map does not have", tag_id(id)))
                            .at(ctx, weap, model_path),
                    ),
                    Some(t) if t.group != FourCC(*b"mode") => out.push(
                        Finding::new(
                            "wrong_group",
                            "error",
                            format!("{model_path} names a {}, not a mode", group_text(t.group)),
                        )
                        .at(ctx, weap, model_path),
                    ),
                    Some(_) => {
                        if let Err(err) = preview::render_model(map, files, id) {
                            out.push(
                                Finding::new("first_person_model_fails", "error", format!("{model_path} ({}) does not load: {err}", name(ctx, id)))
                                    .at(ctx, weap, model_path),
                            );
                        }
                    }
                },
            }
            let anim_path = format!("first_person[{i}].animations");
            match tag_ref(e, 8) {
                None => out.push(
                    Finding::new(
                        "first_person_no_animations",
                        "warning",
                        format!("{anim_path} is null: the first-person model is not posed"),
                    )
                    .at(ctx, weap, anim_path),
                ),
                Some(id) if ctx.catalog.get(id).is_none() => out.push(
                    Finding::new("unresolved_link", "error", format!("{anim_path} names {}, which the map does not have", tag_id(id)))
                        .at(ctx, weap, anim_path),
                ),
                Some(_) => {}
            }
        }
        if let Some(hud) = tag_ref(data, WEAP_HUD)
            && ctx.catalog.get(hud).is_none()
        {
            out.push(
                Finding::new("unresolved_link", "error", format!("hud names {}, which the map does not have", tag_id(hud)))
                    .at(ctx, weap, "hud"),
            );
        }
        // A barrel's marker is where its shots leave from.
        for (i, b) in elements(ctx, data, WEAP_BARRELS, BARREL_SIZE).iter().enumerate() {
            if let Some(marker) = string_at(ctx, b, BARREL_MARKER).filter(|s| !s.is_empty())
                && !markers.contains(&marker)
            {
                out.push(
                    Finding::new(
                        "missing_marker",
                        "error",
                        format!("barrels[{i}] fires from marker {marker:?}, which the weapon's render model lacks"),
                    )
                    .at(ctx, weap, format!("barrels[{i}].optional_barrel_marker_name")),
                );
            }
        }
        // Each player biped attaches a held weapon at its hand marker.
        for character in 0..players {
            let Some(unit) = player_unit(ctx, character) else {
                continue;
            };
            let Some(hand) = tag_data(ctx, unit)
                .and_then(|d| string_at(ctx, d, UNIT_RIGHT_HAND))
                .filter(|s| !s.is_empty())
            else {
                out.push(
                    Finding::new(
                        "missing_marker",
                        "warning",
                        format!("player {character}'s unit {} names no right-hand marker: a held weapon has nothing to attach to", name(ctx, unit)),
                    )
                    .at(ctx, unit, "right_hand_node"),
                );
                continue;
            };
            let biped_markers = ctx
                .resolve_object(unit)
                .ok()
                .and_then(|o| o.model)
                .and_then(|m| m.render_model)
                .and_then(|l| preview::markers(ctx, l.id).ok());
            if let Some(list) = biped_markers
                && !list.iter().any(|mk| mk.name == hand)
            {
                out.push(
                    Finding::new(
                        "missing_marker",
                        "warning",
                        format!("player {character}'s unit {} holds weapons at {hand:?}, which its render model lacks", name(ctx, unit)),
                    )
                    .at(ctx, unit, "right_hand_node"),
                );
            }
        }
    }

    /// The map's raw resources for the geometry decoder.
    struct Raw<'a>(&'a mut assets::ResourceFiles);

    impl tags::geometry::RawSource for Raw<'_> {
        fn read_raw(&mut self, pointer: map::RawPointer, size: u32) -> tags::Result<Vec<u8>> {
            self.0
                .read(pointer.file, pointer.offset, size)
                .map_err(|_| tags::Error::MissingResource(pointer.file))
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
