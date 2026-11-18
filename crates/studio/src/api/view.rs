//! `view.*`: the viewport camera, and offscreen snapshots of the open map
//! (its textured world and placed models when it has them, else the grey
//! world and markers) through the viewport's own renderer, with no window.

use std::path::PathBuf;

use anyhow::{Context as _, Result, anyhow};
use scenario_edit::geometry::Camera;
use scenario_edit::render::{self, Viewport};
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::params::vec3;
use super::{P, Reply, err};
use crate::App;
use crate::app::Action;

/// A wgpu device for offscreen renders, made on the first.
pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

impl Gpu {
    pub fn new() -> Result<Self> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .map_err(|e| anyhow!("no wgpu adapter: {e}"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))
            .context("request a wgpu device")?;
        Ok(Gpu { device, queue })
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
    Ok(camera_json(app.session.camera()))
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
    let mut cam = app.session.camera().clone();
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
    let session = &app.session;
    let mut vp = Viewport::new(&gpu.device);
    match session.scene() {
        Some(scene) => {
            vp.set_scene(&gpu.device, &gpu.queue, scene);
            vp.set_placements(&gpu.device, scene, session.markers(), session.selected());
        }
        None => {
            vp.set_world(&gpu.device, session.world().0);
            vp.set_markers(
                &gpu.device,
                &render::marker_vertices(session.markers(), session.selected()),
            );
        }
    }
    vp.render(&gpu.device, &gpu.queue, &cam, size);
    let texture = vp
        .color_texture()
        .ok_or_else(|| err(code::FAILED, "the viewport drew nothing"))?;
    let rgba = crate::headless::read_texture(&gpu.device, &gpu.queue, texture)
        .map_err(|e| err(code::FAILED, format!("{e:#}")))?;
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
