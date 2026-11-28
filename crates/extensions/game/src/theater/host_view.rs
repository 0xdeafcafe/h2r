//! A playerless host has no retail director or D3D views. Draw an independent
//! desktop view from its re-simulated player datums, posed objects and mounted
//! map. No observer texture, camera or device is visible to the guest.
use super::{FilmCamera, Mode, Picture, Session, camera::ObserverEye};
use crate::spectator_view::horizontal_fov;
use crate::native_map::{self, NativeMap};
use crate::native_world_glue::{self as world_glue, Raw, ReadMemory};
use crate::telemetry::layout;
use guest_layout::view::{Tags, objects, players};
use guest_layout::{DatumHandle, GuestMemory};
use gfx::presenter::OffscreenTarget;
use native_render::camera::RenderCamera;
use native_render::gpu::{DepthRule, GpuWorld, Image, PassViewport};
use native_render::instances::ObjectInput;
use native_render::load;
use native_render::model_gpu::GpuModels;
use native_render::objects::ObjectFrame;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq, Eq)]
struct Dimensions {
    raster: (u32, u32),
    display: (u32, u32),
}
type Present = Box<dyn FnMut(&wgpu::TextureView, (u32, u32)) -> Result<(), String> + Send>;

pub struct HostView {
    gfx: gfx::GfxContext,
    raster: crate::run_opts::NativeSize,
    display: Box<dyn Fn((u32, u32)) -> (u32, u32) + Send>,
    present: Present,
}

impl HostView {
    pub fn new(
        gfx: gfx::GfxContext,
        raster: crate::run_opts::NativeSize,
        display: impl Fn((u32, u32)) -> (u32, u32) + Send + 'static,
        present: impl FnMut(&wgpu::TextureView, (u32, u32)) -> Result<(), String> + Send + 'static,
    ) -> Self {
        Self {
            gfx,
            raster,
            display: Box::new(display),
            present: Box::new(present),
        }
    }

    fn dimensions(&self) -> Dimensions {
        let (w, h) = (self.raster)();
        let limit = self
            .gfx
            .device
            .limits()
            .max_texture_dimension_2d
            .min(settings::Resolution::MAX_DIMENSION);
        let longest = w.max(h).max(limit);
        let axis = |n| {
            ((u64::from(n) * u64::from(limit) + u64::from(longest / 2)) / u64::from(longest)).max(1)
                as u32
        };
        let raster = (axis(w), axis(h));
        Dimensions {
            raster,
            display: (self.display)(raster),
        }
    }
}

pub(super) struct MountedView {
    output: HostView,
    vfs: Arc<Mutex<vfs::Vfs>>,
    target: OffscreenTarget,
    pool: gfx::ring::UploadPool,
    map: Option<(Bsp, Option<Scene>)>,
    last: Option<(u64, u64, Dimensions)>,
    error: Option<String>,
    unavailable: Vec<String>,
    blank: bool,
}

impl Session {
    pub fn attach_host_view(
        &self,
        output: HostView,
        vfs: Arc<Mutex<vfs::Vfs>>,
    ) -> anyhow::Result<()> {
        if let Mode::Replay(replay) = &self.lock().mode {
            replay.camera.set_playerless();
        }
        let target = OffscreenTarget::new(
            &output.gfx.device,
            output.dimensions().raster,
            wgpu::TextureFormat::Rgba8Unorm,
        );
        self.view
            .set(Mutex::new(MountedView {
                output,
                vfs,
                target,
                pool: Default::default(),
                map: None,
                last: None,
                error: None,
                unavailable: Vec::new(),
                blank: false,
            }))
            .map_err(|_| anyhow::anyhow!("film observer already attached"))
    }

    pub(super) fn draw_host_view(&self, rt: &runtime::Runtime) {
        let Some(view) = self.view.get() else { return };
        let (tick, every, paused, camera) = {
            let s = self.lock();
            let Mode::Replay(r) = &s.mode else { return };
            if s.seeking.is_some()
                || s.failure.is_some()
                || s.restart.is_some()
                || r.render_every == 0
            {
                return;
            }
            (s.tick, r.render_every, s.paused, r.camera.clone())
        };
        let revision = camera.revision();
        let mut view = view.lock().unwrap_or_else(|e| e.into_inner());
        let dimensions = view.output.dimensions();
        if let Some((last_tick, last_revision, last_size)) = view.last
            && revision == last_revision
            && dimensions == last_size
            && (tick == last_tick || (!paused && !tick.is_multiple_of(u64::from(every))))
        {
            return;
        }
        view.last = Some((tick, revision, dimensions));
        // Release the guest memory gate before reading files, uploading or presenting.
        let mut unavailable = Vec::new();
        let frame = Frame::read(
            &rt.memory(),
            view.map.as_ref().map(|(bsp, _)| bsp),
            &mut unavailable,
        );
        if unavailable != view.unavailable {
            for message in &unavailable {
                if !view.unavailable.contains(message) {
                    eprintln!("theater: {message}");
                }
            }
            view.unavailable = unavailable;
        }
        if let Err(e) = view.draw(frame, &camera, dimensions, None) {
            if view.error.as_deref() != Some(&e) {
                eprintln!("theater: playerless observer: {e}");
                view.error = Some(e);
            }
        }
    }

    pub(super) fn capture_scaled_host_view(
        &self,
        rt: &runtime::Runtime,
        raster: (u32, u32),
    ) -> anyhow::Result<Picture> {
        let camera = {
            let state = self.lock();
            let Mode::Replay(replay) = &state.mode else {
                anyhow::bail!("screenshot needs replay");
            };
            replay.camera.clone()
        };
        let mut view = self.view.get().ok_or_else(|| anyhow::anyhow!("no replay observer"))?
            .lock().unwrap_or_else(|e| e.into_inner());
        let limit = view.output.gfx.device.limits().max_texture_dimension_2d;
        anyhow::ensure!(raster.0 <= limit && raster.1 <= limit,
            "scaled screenshot exceeds the GPU render size limit ({limit})");
        let dimensions = Dimensions { raster, display: view.output.dimensions().display };
        let frame = Frame::read(
            &rt.memory(),
            view.map.as_ref().map(|(bsp, _)| bsp),
            &mut Vec::new(),
        );
        let mut picture = None;
        view.draw(frame, &camera, dimensions, Some(&mut picture)).map_err(anyhow::Error::msg)?;
        picture.ok_or_else(|| anyhow::anyhow!("no scaled screenshot was rendered"))
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Bsp {
    map: String,
    source: Option<String>,
    checksum: u32,
    index: u16,
    address: u32,
}

struct Eye {
    seat: u8,
    unit: u32,
    weapons: [u32; 4],
    facing: [f32; 3],
    camera: RenderCamera,
}

struct Frame {
    bsp: Bsp,
    eyes: Vec<Eye>,
    objects: Vec<ObjectFrame>,
}

impl Frame {
    fn read(
        g: &runtime::Guest,
        previous: Option<&Bsp>,
        unavailable: &mut Vec<String>,
    ) -> Option<Self> {
        let eyes: Vec<_> = players::array(g)
            .ok()??
            .live(g, 16)
            .ok()?
            .into_iter()
            .filter_map(|(slot, p)| {
                if !p.listed(g).ok()? {
                    return None;
                }
                let unit = p.unit(g).ok()??;
                let seat = slot.index() as u8;
                match Self::eye(g, seat, unit) {
                    Ok(eye) => Some(eye),
                    Err(why) => {
                        unavailable.push(format!("no first-person view for seat {seat}: {why}"));
                        None
                    }
                }
            })
            .collect();
        if eyes.is_empty() && previous.is_none() {
            return None;
        }
        let map = crate::telemetry::loaded_map_name(g).unwrap_or_default();
        let address = g.u32(render_bridge::native_world::ACTIVE_BSP).ok();
        let index = world_glue::active_bsp_index(g);
        let (Some(index), Some(address)) = (index, address.filter(|&a| a != 0)) else {
            if !eyes.is_empty() {
                unavailable.push(format!("map {map:?} has player views but no active BSP"));
            }
            return None;
        };
        if map.is_empty() {
            if !eyes.is_empty() {
                unavailable
                    .push("player views are ready but the active cache has no map name".into());
            }
            return None;
        }
        let bsp = Bsp {
            map,
            address,
            index,
            source: crate::map_slots::active_file(g),
            checksum: guest_layout::game::cache_checksum(g as &dyn guest_layout::GuestMemory).unwrap_or_default(),
        };
        // While every player is respawning, continue rendering a detached
        // camera in the last map it actually saw. Never open a menu BSP for it.
        if eyes.is_empty() && previous != Some(&bsp) {
            return None;
        }
        let mem = ReadMemory(g);
        let objects = objects::array(g)
            .ok()??
            .live(g, bots::vehicle::read::OBJECT_LIMIT)
            .ok()?
            .into_iter()
            .filter_map(|(h, _)| native_render::objects::read_object(&mem, h.raw()))
            .collect();
        Some(Self { bsp, eyes, objects })
    }

    fn eye(g: &runtime::Guest, seat: u8, unit: DatumHandle) -> Result<Eye, &'static str> {
        let (_, body) = objects::get(g, unit)
            .ok()
            .flatten()
            .ok_or("unit body unavailable")?;
        let mut forward = GuestMemory::vec3(g, body.addr + layout::UNIT_AIM)
            .map_err(|_| "unit aim unreadable")?;
        let length = forward.iter().map(|x| x * x).sum::<f32>().sqrt();
        if !(0.75..=1.25).contains(&length) {
            return Err("unit aim is not a direction");
        }
        forward.iter_mut().for_each(|v| *v /= length);
        let tag = body
            .definition(g)
            .ok()
            .flatten()
            .ok_or("unit definition unavailable")?;
        let definition = Tags
            .entry(g, tag)
            .ok()
            .flatten()
            .and_then(|t| t.address(g).ok())
            .ok_or("unit tag unavailable")?;
        let fov = GuestMemory::f32(g, definition + crate::fov_row::UNIT_CAMERA_FOV)
            .map_err(|_| "unit FOV unreadable")?;
        if !(0.01..std::f32::consts::PI).contains(&fov) {
            return Err("unit FOV outside valid range");
        }
        let crouch = GuestMemory::f32(g, body.addr + layout::UNIT_CROUCH).unwrap_or(f32::NAN);
        let position = bots::vehicle::read::unit_eye(g, unit, crouch)
            .ok()
            .flatten()
            .ok_or("no posed camera marker or usable biped collision height")?;
        let facing = body.forward(g).map_err(|_| "unit facing unreadable")?;
        let up = if forward[2].abs() < 0.9999 {
            [0., 0., 1.]
        } else {
            // Keep a usable basis at straight up/down, facing with the body.
            [-forward[2] * facing[0], -forward[2] * facing[1], 0.]
        };
        let weapons = std::array::from_fn(|i| {
            g.u32(body.addr + layout::UNIT_WEAPONS + 4 * i as u32)
                .unwrap_or(u32::MAX)
        });
        Ok(Eye {
            seat,
            unit: unit.raw(),
            weapons,
            facing,
            camera: RenderCamera {
                position,
                forward,
                up,
                // The unit tag gives the retail horizontal FOV of a 4:3 view.
                fov: retail_vertical_fov(fov),
                near: 0.03,
                far: 4096.,
                projection_xy: None,
                viewport: [0, 0, 480, 640],
            },
        })
    }
}

struct Scene {
    world: GpuWorld,
    models: GpuModels,
    map: NativeMap,
    ctx: tags::object::Context,
    raw: Raw,
    uploaded: HashMap<u32, Option<u32>>,
    previous_view: Option<RenderCamera>,
}

fn textures(
    map: &mut NativeMap,
    raw: &mut Raw,
    shaders: &[map::TagId],
    materials: &[u16],
) -> HashMap<u16, Image> {
    materials
        .iter()
        .filter_map(|&m| {
            let bitmap = load::base_map(map, *shaders.get(usize::from(m))?)?;
            Some((m, load::decode_first(map, &mut raw.0, bitmap)?))
        })
        .collect()
}

impl Scene {
    fn load(gfx: &gfx::GfxContext, vfs: &Mutex<vfs::Vfs>, bsp: &Bsp) -> Result<Self, load::Error> {
        let (mut map, files) =
            native_map::open_source_from_vfs(vfs, &bsp.map, bsp.source.as_deref())?;
        let bsps = load::scenario_bsps(&mut map)?;
        let index = usize::from(bsp.index);
        let (id, _) = bsps.get(index).ok_or("BSP index outside scenario")?;
        if map.tag(*id)?.location.as_ref().map(|l| l.address) != Some(bsp.address) {
            return Err("loaded BSP address differs from the scenario".into());
        }
        let mut raw = Raw(files);
        let loaded = load::load_world(&mut map, &mut raw, index)?;
        let mut base = textures(
            &mut map,
            &mut raw,
            &loaded.shaders,
            &loaded.world.materials(),
        );
        let bitmap = loaded
            .lightmap_bitmap
            .and_then(|id| tags::DecodedTag::read(&mut map, id).ok())
            .and_then(|tag| {
                if let tags::TagData::Bitmap(b) = tag.data {
                    Some(b)
                } else {
                    None
                }
            });
        let pages: HashMap<_, _> = loaded
            .world
            .lightmap_keys()
            .into_iter()
            .filter_map(|k| {
                let image = bitmap.as_ref()?.images.get(usize::from(k.page))?;
                let palette = load::page_palette(&loaded.palettes, k).ok()?;
                Some((
                    k,
                    load::decode_lightmap(image, palette, &mut raw.0, false).ok()?,
                ))
            })
            .collect();
        let world = native_render::world::keep_draws(loaded.world, &|d| {
            base.contains_key(&d.material) && d.lightmap.is_none_or(|k| pages.contains_key(&k))
        });
        if world.draws.is_empty() {
            return Err("BSP has no renderable textured geometry".into());
        }
        let world = GpuWorld::new(
            &gfx.device,
            &gfx.queue,
            world,
            &mut |m| base.remove(&m),
            &mut |k| pages.get(&k).cloned(),
        );
        let ctx = tags::object::Context::read(&mut map)?;
        Ok(Self {
            world,
            models: GpuModels::new(&gfx.device, &gfx.queue),
            map,
            ctx,
            raw,
            uploaded: Default::default(),
            previous_view: None,
        })
    }

    fn models(&mut self, gfx: &gfx::GfxContext, objects: &[ObjectFrame]) {
        for obj in objects {
            if self.uploaded.contains_key(&obj.definition) {
                continue;
            }
            let model =
                native_map::tag_of(&self.map, obj.definition).and_then(|id| {
                    let loaded = native_render::model_load::load_object_model(
                        &mut self.map,
                        &self.ctx,
                        &mut self.raw,
                        id,
                    )
                    .ok()?;
                    let mut model = loaded.model;
                    let mut images = textures(
                        &mut self.map,
                        &mut self.raw,
                        &loaded.shaders,
                        &model.materials(),
                    );
                    model.draws.retain(|d| images.contains_key(&d.material));
                    (!model.draws.is_empty()).then(|| {
                        self.models
                            .add_model(&gfx.device, &gfx.queue, model, &mut |m| images.remove(&m))
                    })
                });
            self.uploaded.insert(obj.definition, model);
        }
    }
}

fn retail_vertical_fov(horizontal: f32) -> f32 {
    2.0 * ((horizontal * 0.5).tan() * (3.0 / 4.0)).atan()
}

fn observer_lens(camera: RenderCamera, (w, h): (u32, u32)) -> RenderCamera {
    let factor = camera.aspect() * h.max(1) as f32 / w.max(1) as f32;
    native_render::camera::display_camera(camera, factor)
}

impl MountedView {
    fn draw(
        &mut self,
        frame: Option<Frame>,
        camera: &FilmCamera,
        dimensions: Dimensions,
        capture: Option<&mut Option<Picture>>,
    ) -> Result<(), String> {
        let gfx = &self.output.gfx;
        if self.target.target().size != dimensions.raster {
            self.target = OffscreenTarget::new(
                &gfx.device,
                dimensions.raster,
                wgpu::TextureFormat::Rgba8Unorm,
            );
            self.blank = false;
        }
        let target = self.target.target();
        let depth = target.depth.expect("offscreen depth");
        let depth_format = gfx::presenter::DEPTH_FORMAT;
        let mut visible = None;
        if let Some(frame) = &frame {
            if self.map.as_ref().is_none_or(|(b, _)| b != &frame.bsp) {
                let loaded = Scene::load(gfx, &self.vfs, &frame.bsp);
                let error = loaded
                    .as_ref()
                    .err()
                    .map(|e| format!("{} BSP {}: {e}", frame.bsp.map, frame.bsp.index));
                self.map = Some((frame.bsp.clone(), loaded.ok()));
                if let Some(e) = error {
                    eprintln!("theater: {e}")
                }
            }
            if let Some((_, Some(scene))) = &mut self.map {
                let eyes = frame.eyes.iter().map(|e| ObserverEye {
                    seat: e.seat,
                    position: e.camera.position,
                    aim: e.camera.forward,
                    facing: e.facing,
                    fov: horizontal_fov(e.camera.fov),
                });
                if let Some(rig) = camera.host_pose(eyes) {
                    let eye = rig.hide_subject.and_then(|seat| frame.eyes.iter().find(|e| e.seat == seat));
                    let lens = eye.or_else(|| frame.eyes.first()).map(|e| e.camera)
                        .or(scene.previous_view)
                        .unwrap_or(RenderCamera {
                            position: rig.position,
                            forward: rig.forward,
                            up: [0., 0., 1.],
                            fov: rig.fov,
                            projection_xy: None,
                            viewport: [0, 0, 480, 640],
                            near: 0.03,
                            far: 4096.,
                        });
                    let up = if eye.is_some() {
                        lens.up
                    } else if rig.forward[2].abs() > 0.9999 {
                        [1., 0., 0.]
                    } else {
                        [0., 0., 1.]
                    };
                    let view = RenderCamera {
                        position: rig.position,
                        forward: rig.forward,
                        up,
                        fov: rig.fov,
                        ..lens
                    };
                    scene.previous_view = Some(view);
                    let view = observer_lens(view, dimensions.display);
                    scene.models(gfx, &frame.objects);
                    visible = Some((scene, view, eye, &frame.objects));
                }
            } else {
                let _ = camera.host_pose(std::iter::empty());
            }
        } else {
            let _ = camera.host_pose(std::iter::empty());
        }
        // A map transition or lost first-person subject clears the last POV.
        let blank = visible.is_none();
        if blank && self.blank && capture.is_none() {
            return Ok(());
        }
        let mut encoder = gfx.device.create_command_encoder(&Default::default());
        drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Theater observer clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target.color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0),
                    store: wgpu::StoreOp::Store,
                }),
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        }));
        let arena = self.pool.begin();
        if let Some((scene, view, hidden, objects)) = visible {
            let instances: Vec<_> = objects
                .iter()
                .filter(|o| {
                    hidden
                        .is_none_or(|eye| o.handle != eye.unit && !eye.weapons.contains(&o.handle))
                })
                .filter_map(|o| {
                    Some(ObjectInput {
                        model: (*scene.uploaded.get(&o.definition)?)?,
                        nodes: &o.nodes,
                        sphere: o.sphere,
                        highlight: [0; 4],
                    })
                })
                .collect();
            let mut uploads = gfx::ring::EncoderUploads {
                device: &gfx.device,
                queue: &gfx.queue,
                encoder: &mut encoder,
                arena: &arena,
            };
            let viewport = PassViewport {
                x: 0.,
                y: 0.,
                width: dimensions.raster.0 as f32,
                height: dimensions.raster.1 as f32,
            };
            scene.world.prepare(&mut uploads, &view, viewport, DepthRule::D24);
            scene
                .models
                .prepare(&gfx.device, &mut uploads, &view, DepthRule::D24, &instances);
            scene.world.encode(
                &gfx.device,
                &mut encoder,
                target.color,
                target.color_format,
                depth,
                depth_format,
                viewport,
                dimensions.raster,
            );
            scene.models.encode(
                &gfx.device,
                &mut encoder,
                target.color,
                target.color_format,
                depth,
                depth_format,
                viewport,
            );
        }
        let patches = arena.upload_commands(&gfx.queue);
        gfx.queue
            .submit(patches.into_iter().chain([encoder.finish()]));
        gfx.queue.on_submitted_work_done(move || drop(arena));
        if let Some(capture) = capture {
            let mut rgba = self.target.read_pixels(gfx);
            for pixel in rgba.chunks_exact_mut(4) {
                pixel[3] = 255;
            }
            *capture = Some(Picture { size: dimensions.raster, rgba });
        } else {
            (self.output.present)(target.color, dimensions.raster)?;
            self.blank = blank;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_render::{cull::Frustum, world::Aabb};

    #[test]
    fn observer_projection_and_culling_admit_the_same_periphery_at_each_aspect() {
        let eye = RenderCamera {
            position: [0.; 3],
            forward: [1., 0., 0.],
            up: [0., 0., 1.],
            fov: retail_vertical_fov(90f32.to_radians()),
            projection_xy: None,
            viewport: [0, 0, 480, 640],
            near: 0.03,
            far: 4096.,
        };
        for ((w, h), edge) in [
            ((640, 480), 10.),
            ((1920, 1200), 12.),
            ((1920, 1080), 40. / 3.),
            ((2520, 1080), 17.5),
            ((5120, 1440), 80. / 3.),
        ] {
            let vp = observer_lens(eye, (w, h)).view_projection();
            let frustum = Frustum::from_view_projection(&vp);
            for (side, up, visible) in [
                (edge - 0.1, 0., true),
                (edge + 0.1, 0., false),
                (0., 7.4, true),
                (0., 7.6, false),
            ] {
                let p = [10., -side, up];
                let clip = frame_scene::camera::transform(&vp, [p[0], p[1], p[2], 1.]);
                assert_eq!(
                    clip[0].abs() <= clip[3] && clip[1].abs() <= clip[3],
                    visible
                );
                assert_eq!(frustum.may_see(&Aabb { min: p, max: p }), visible);
            }
        }
    }
}
