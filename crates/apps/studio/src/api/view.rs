//! `view.*`: the viewport camera, and offscreen snapshots of the open map
//! (its textured world and placed models when it has them, else the grey
//! world and markers) through the viewport's own renderer, with no window,
//! and what that renderer draws (`view.stats`: lightmaps, materials, the
//! map's sun and fog).

use std::path::PathBuf;

use anyhow::{Context as _, Result, anyhow};
use scenario_edit::geometry::{BspLighting, Camera};
use scenario_edit::layers::{LayerId, Preset};
use scenario_edit::native_view::Lighting;
use scenario_edit::render::Viewport;
use scenario_edit::scene::{Blend, Scene};
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::params::vec3;
use super::{P, Reply, err};
use crate::App;
use crate::app::Action;

/// A wgpu device for offscreen renders, made on the first.
pub struct Gpu {
    _census: mem_watch::gpu::DeviceRegistration,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

impl Gpu {
    pub fn new() -> Result<Self> {
        let instance = gfx::instance();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .map_err(|e| anyhow!("no wgpu adapter: {e}"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))
            .context("request a wgpu device")?;
        Ok(Gpu {
            _census: mem_watch::gpu::DeviceRegistration::new(&instance, &adapter, &device),
            device,
            queue,
        })
    }
}

fn camera_json(c: &Camera) -> Value {
    json!({
        "yaw": c.yaw.to_degrees(),
        "pitch": c.pitch.to_degrees(),
        "distance": c.distance,
        "target": c.target,
    })
}

/// Applies `yaw`, `pitch` (degrees), `distance` and `target` from `p`.
fn apply_camera(c: &mut Camera, p: &P) -> Result<(), agent_proto::RpcError> {
    if let Some(v) = p.opt_f32("yaw")? {
        c.yaw = v.to_radians();
    }
    if let Some(v) = p.opt_f32("pitch")? {
        c.pitch = v.to_radians().clamp(-1.55, 1.55);
    }
    if let Some(v) = p.opt_f32("distance")? {
        if v <= 0.0 {
            return Err(err(code::INVALID_PARAMS, "distance: more than 0"));
        }
        c.distance = v;
    }
    if let Some(v) = p.get("target") {
        c.target = vec3("target", v)?;
    }
    Ok(())
}

pub fn camera(app: &mut App, p: &P) -> Reply {
    super::scenario::open(app)?;
    let speed = p.opt_f32("fly_speed")?;
    if speed.is_some_and(|s| !(crate::camera::MIN_SPEED..=crate::camera::MAX_SPEED).contains(&s)) {
        return Err(err(code::INVALID_PARAMS, "fly_speed: 0.5..1000 world units per second"));
    }
    if let Some(f) = p.get("focus") {
        let q = P(f);
        let entry = q.entry(app)?;
        app.perform(Action::Select(Some(entry)), false)
            .map_err(super::scenario::refused)?;
        app.perform(Action::FocusSelected, false)
            .map_err(super::scenario::refused)?;
    }
    if p.bool("all")? {
        app.perform(Action::FrameAll, false)
            .map_err(super::scenario::refused)?;
    }
    apply_camera(app.session.camera_mut(), p)?;
    if let Some(speed) = speed {
        app.dispatch(Action::SetFlySpeed(speed));
    }
    let mut reply = camera_json(app.session.camera());
    reply["fly_speed"] = json!(app.fly_speed());
    Ok(reply)
}

pub fn layers(app: &mut App, p: &P) -> Reply {
    // Validate the whole request before applying its changes in one action.
    let mut next = app.layers.clone();
    if let Some(name) = p.opt_str("preset")? {
        let preset = Preset::ALL.into_iter().find(|preset| preset.id() == name)
            .ok_or_else(|| err(code::INVALID_PARAMS, format!("unknown layer preset {name:?}")))?;
        next.apply(preset);
    }
    if let Some(set) = p.get("set") {
        let entries = set.as_object()
            .ok_or_else(|| err(code::INVALID_PARAMS, "set must map layer names to booleans"))?;
        for (name, value) in entries {
            let layer = LayerId::all().find(|id| id.id() == name.as_str())
                .ok_or_else(|| err(code::INVALID_PARAMS, format!("unknown layer {name:?}")))?;
            let on = value.as_bool().ok_or_else(|| {
                err(code::INVALID_PARAMS, format!("set.{name} must be true or false"))
            })?;
            next.set(layer, on);
        }
    }
    if p.has("xray") {
        next.xray = p.bool("xray")?;
    }
    if next != app.layers {
        app.perform(Action::SetLayers(next), false)
            .map_err(super::scenario::refused)?;
    }
    let rows: Vec<Value> = LayerId::all()
        .map(|id| json!({
            "name": id.id(),
            "label": id.label(),
            "group": id.group(),
            "on": app.layers.visible(id),
            "count": app.layer_count(id),
            "color": crate::theme::layer_color(id),
        }))
        .collect();
    Ok(json!({ "layers": rows, "xray": app.layers.xray }))
}

pub fn snapshot(gpu: &Gpu, app: &mut App, p: &P) -> Reply {
    super::scenario::open(app)?;
    let size = match p.get("size") {
        None => [960, 540],
        Some(v) => {
            let a: Option<Vec<u32>> = v.as_array().map(|a| {
                a.iter()
                    .filter_map(|x| x.as_u64().and_then(|n| u32::try_from(n).ok()))
                    .collect()
            });
            match a.as_deref() {
                Some(&[w, h]) if (1..=8192).contains(&w) && (1..=8192).contains(&h) => [w, h],
                _ => {
                    return Err(err(
                        code::INVALID_PARAMS,
                        format!("size: [w, h] of 1..8192, not {v}"),
                    ));
                }
            }
        }
    };
    let mut cam = *app.session.camera();
    if let Some(c) = p.get("camera") {
        apply_camera(&mut cam, &P(c))?;
    }
    let out = match p.opt_str("out")? {
        Some(o) => PathBuf::from(o),
        None => studio_proto::output_dir()
            .join("view")
            .join(format!("{}.png", app.loaded_map().unwrap_or("map"))),
    };
    super::docs::outside_git(&out)?;
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| err(code::FAILED, format!("{}: {e}", dir.display())))?;
    }
    let live = crate::viewport::mirror::Scene::shown(app);
    let session = &app.session;
    let mut vp = Viewport::new(&gpu.device);
    vp.set_layers(&app.layers);
    match session.scene() {
        Some(scene) => vp.set_scene(&gpu.device, &gpu.queue, scene),
        None => vp.set_world(&gpu.device, session.world().0),
    }
    let native = session.native_world().map_or(&[][..], |n| n.as_slice());
    vp.set_lighting(&gpu.device, &gpu.queue, native, app.lighting);
    vp.set_native(&gpu.device, &gpu.queue, native);
    let models = session.native_models().cloned().unwrap_or_default();
    vp.set_native_models(&gpu.device, &gpu.queue, models);
    crate::viewport::upload_placements(&mut vp, &gpu.device, app, &live.linked);
    crate::viewport::upload_overlays(&mut vp, &gpu.device, app);
    vp.set_live_instances(&gpu.device, &gpu.queue, session.source(), &live.instances);
    vp.render(&gpu.device, &gpu.queue, &cam, size);
    let texture = vp
        .color_texture()
        .ok_or_else(|| err(code::FAILED, "the viewport drew nothing"))?;
    let mut rgba = crate::headless::read_texture(&gpu.device, &gpu.queue, texture)
        .map_err(|e| err(code::FAILED, format!("{e:#}")))?;
    let labels = crate::viewport::mirror::gameplay_labels(app);
    crate::viewport::mirror::snapshot_labels(&mut rgba, size, &cam, &labels);
    live.snapshot(&mut rgba, size, &cam);
    crate::headless::write_png(&out, &rgba, texture.width(), texture.height())
        .map_err(|e| err(code::FAILED, format!("{e:#}")))?;
    let _ = m::VIEW_SNAPSHOT;
    Ok(json!({
        "path": out.display().to_string(),
        "width": texture.width(),
        "height": texture.height(),
        "camera": camera_json(&cam),
    }))
}

/// `view.lighting`: the viewport's Lighting mode, set by `mode` (an id
/// of [`Lighting`]) when given.
pub fn lighting(app: &mut App, p: &P) -> Reply {
    let _ = m::VIEW_LIGHTING;
    if let Some(id) = p.opt_str("mode")? {
        let mode = Lighting::from_id(id).ok_or_else(|| {
            let ids: Vec<_> = Lighting::ALL.iter().map(|l| l.id()).collect();
            err(
                code::INVALID_PARAMS,
                format!("mode: one of {}, not {id}", ids.join(", ")),
            )
        })?;
        app.perform(Action::SetLighting(mode), false)
            .map_err(super::scenario::refused)?;
    }
    Ok(json!({
        "mode": app.lighting.id(),
        "modes": Lighting::ALL.map(Lighting::id),
    }))
}

/// `view.stats`: what the viewport draws for the open map
/// ([`scene_stats`]); `{"scene": false}` while it shows the grey world.
/// `native` is the level on the native renderer ([`native_stats`]).
pub fn stats(app: &mut App, _p: &P) -> Reply {
    super::scenario::open(app)?;
    let _ = m::VIEW_STATS;
    let mut out = match app.session.scene() {
        Some(scene) => scene_stats(scene),
        None => json!({ "scene": false }),
    };
    out["native"] = native_stats(app);
    Ok(out)
}

/// The open map's level on the native renderer: the Lighting mode, each
/// BSP's draws, batches, materials, lightmap pages, draws left out for a
/// missing picture and sections that did not decode, and why any BSP did
/// not load.
pub fn native_stats(app: &App) -> Value {
    let session = &app.session;
    let bsps: Vec<Value> = session
        .native_world()
        .map(|n| n.as_slice())
        .unwrap_or_default()
        .iter()
        .map(|l| {
            let s = l.stats();
            json!({
                "bsp": l.bsp.0,
                "draws": s.draws,
                "batches": s.batches,
                "materials": s.materials,
                "lightmap_pages": s.lightmap_pages,
                "untextured": s.untextured,
                "failed_sections": s.failed_sections,
                "lightmap_error": l.lightmap_error,
            })
        })
        .collect();
    let errors: Vec<Value> = session
        .native_errors()
        .iter()
        .map(|(index, why)| json!({ "index": index, "error": why }))
        .collect();
    // The placed objects: how many tags have a model, how many
    // placements draw one, and each tag left to the scene and why.
    let objects = session.native_models().map(|models| {
        let placed = session
            .markers()
            .iter()
            .filter(|m| m.object.is_some_and(|o| models.of(o).is_some()))
            .count();
        let refused: Vec<Value> = models
            .errors
            .iter()
            .map(|(tag, why)| json!({ "tag": tag.0, "error": why }))
            .collect();
        json!({ "models": models.len(), "placed": placed, "errors": refused })
    });
    json!({
        "lighting": app.lighting.id(),
        "bsps": bsps,
        "errors": errors,
        "objects": objects,
    })
}

/// A scene's render stats: the counts, each BSP's lighting, the world's
/// triangles by blend and how many solid ones a lightmap lights, every
/// material with its template, blend and picture, and the sky's sun and
/// fog.
pub fn scene_stats(scene: &Scene) -> Value {
    let st = &scene.stats;
    let mut by_blend = serde_json::Map::new();
    for b in Blend::ALL {
        let n: usize = scene
            .world
            .batches
            .iter()
            .filter(|x| x.blend == b)
            .map(|x| x.indices.len() / 3)
            .sum();
        by_blend.insert(b.name().into(), json!(n));
    }
    let (lit, all) = scene
        .world
        .batches
        .iter()
        .filter(|b| !b.blend.see_through())
        .fold((0usize, 0usize), |(l, a), b| {
            let n = b.indices.len() / 3;
            (l + if b.lightmap.is_some() { n } else { 0 }, a + n)
        });
    let a = &scene.atmosphere;
    let bsps: Vec<Value> = scene
        .lighting
        .iter()
        .map(|l| {
            let detail = match &l.lighting {
                BspLighting::ChecksumMismatch { group, bsp } => {
                    Some(format!("group checksum {group:#010x}, BSP {bsp:#010x}"))
                }
                BspLighting::Unreadable(e) => Some(e.clone()),
                _ => None,
            };
            json!({
                "bsp": l.bsp,
                "lighting": l.lighting.name(),
                "detail": detail,
            })
        })
        .collect();
    let materials: Vec<Value> = scene
        .materials
        .iter()
        .map(|m| {
            json!({
                "shader": m.shader,
                "template": m.template,
                "blend": m.blend.name(),
                "blend_from_passes": m.blend_from_passes,
                "emissive": m.emissive,
                "base": m.base,
                "details": m.details,
            })
        })
        .collect();
    let sun = a.sun.map(|s| {
        json!({
            "direction": s.direction,
            "color": s.color,
            "power": s.power,
        })
    });
    let fog = a.fog.map(|f| {
        json!({
            "color": f.color,
            "density": f.density,
            "start": f.start,
            "opaque": f.opaque,
        })
    });
    json!({
        "scene": true,
        "shaders": st.shaders,
        "textured": st.textured,
        "bitmaps_failed": st.bitmaps_failed,
        "lightmap_pages": st.lightmap_pages,
        "objects": st.objects,
        "modelled": st.modelled,
        "bsps": bsps,
        "triangles": by_blend,
        "solid_triangles": all,
        "lightmapped_triangles": lit,
        "materials": materials,
        "sky": scene.sky.as_ref().map(|s| s.name.clone()),
        "atmosphere": {
            "sky": a.sky,
            "sun": sun,
            "fog": fog,
            "fallback_rgb": a.fallback_rgb,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use scenario_edit::geometry::WorldPart;
    use scenario_edit::scene::{Atmosphere, BspLightingInfo, Fog, MaterialInfo, Sun};

    fn tri() -> WorldPart {
        WorldPart {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            uvs: vec![[0.0; 2]; 3],
            lightmap_uvs: vec![[0.5; 2]; 3],
            tris: vec![[0, 1, 2]],
            ..WorldPart::default()
        }
    }

    #[test]
    fn view_layers_counts_translucent_structure_even_when_hidden() {
        let mut scene = Scene::default();
        scene.world.push(None, None, Blend::Translucent, &tri());
        let mut app = App::new();
        app.session.open_reference(scenario_edit::Reference {
            scene: Some(std::sync::Arc::new(scene)),
            ..Default::default()
        });
        app.layers.set(LayerId::Structure, false);
        assert_eq!(app.layer_count(LayerId::Structure), 1);
    }

    /// The stats say how each BSP is lit, count the world's triangles by
    /// blend and the solid ones a lightmap lights, list the materials and
    /// carry the sky's sun and fog.
    #[test]
    fn scene_stats_report_lighting_blends_materials_and_fog() {
        let mut scene = Scene::default();
        scene.world.push(Some(0), Some(1), Blend::Opaque, &tri());
        scene.world.push(Some(0), None, Blend::Opaque, &tri());
        scene.world.push(Some(0), None, Blend::Additive, &tri());
        scene.stats.lightmap_pages = 1;
        scene.lighting.push(BspLightingInfo {
            bsp: r"scenarios\ui\mainmenu\mainmenu".into(),
            lighting: BspLighting::EmptyGroup,
        });
        scene.materials.push(MaterialInfo {
            shader: "light_long_volume".into(),
            template: r"shaders\shader_templates\transparent\one_add_illum_detail".into(),
            blend: Blend::Additive,
            blend_from_passes: true,
            emissive: false,
            base: Some("light_volume".into()),
            details: 0,
        });
        scene.atmosphere = Atmosphere {
            sky: Some("main_menu".into()),
            sun: Some(Sun {
                direction: [0.0, 0.0, 1.0],
                color: [1.0, 0.5, 0.25],
                power: 1.5,
            }),
            fog: Some(Fog {
                color: [0.5, 0.5, 0.75],
                density: 0.5,
                start: 10.0,
                opaque: 300.0,
            }),
            fallback_rgb: Some([0.25, 0.5, 0.75]),
        };
        let v = scene_stats(&scene);
        assert_eq!(v["scene"], true);
        assert_eq!(v["lightmap_pages"], 1);
        assert_eq!(v["bsps"][0]["lighting"], "empty lightmap group");
        assert_eq!(v["triangles"]["opaque"], 2);
        assert_eq!(v["triangles"]["additive"], 1);
        assert_eq!(v["triangles"]["translucent"], 0);
        assert_eq!(v["solid_triangles"], 2);
        assert_eq!(v["lightmapped_triangles"], 1);
        assert_eq!(v["materials"][0]["blend"], "additive");
        assert_eq!(v["materials"][0]["blend_from_passes"], true);
        assert_eq!(v["atmosphere"]["fog"]["density"], 0.5);
        assert_eq!(v["atmosphere"]["sun"]["power"], 1.5);
        assert_eq!(v["atmosphere"]["sky"], "main_menu");
    }
}
