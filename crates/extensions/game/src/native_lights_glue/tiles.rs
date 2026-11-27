//! The held run of slot 4's lights, drawn tiled (docs/graphics/
//! native-light-tiles.md, "In the game"): a light the tiled path admits is
//! held at its passes ([`Glue::hold_light`]); the run draws when the
//! next light draws on its own or the light frame ends
//! ([`Glue::flush_run`]), as one sequence whose step bins the run's
//! lights and draws their layers 12 and 13 instanced ([`TiledRun`]). The
//! compare arm (`native_render::light_tiles::ARM`) draws a compared run
//! per light and the tiled pass into a copy of the target, compared on
//! the GPU ([`CompareRun`]) and read back a few runs later; a difference
//! turns the arm off. A run that does not prepare draws per light.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use native_render::light_tiles::{self, ARM, Decision, MAX_RUN, Segment, TileRefusal, TileVolume};
use native_render::light_tiles_gpu::{self, Compare, LightTiles, LitPass, TiledDraw};
use native_render::program::ViewInputs;
use render_bridge::native_lights::{SceneDraw, ScenePass};

use super::{
    DeviceMemory, Glue, Keep, PreparedLight, RetainedLight, constants, scale_scissor, scene_light,
};

/// One held light: its passes as the scene gave them and its layers 12
/// and 13 prepared at its passes.
pub(super) struct Held {
    passes: Vec<ScenePass>,
    lit: Vec<(u8, PreparedLight)>,
}

/// The run of held lights of one view's target.
#[derive(Default)]
pub(super) struct Run {
    view_target: Option<[u32; 8]>,
    lights: Vec<Held>,
}

/// Each copy's scratch target a compared run draws its tiled pass into.
#[derive(Clone, Default)]
struct Scratch(Arc<Mutex<Vec<(d3d::TargetCopy, wgpu::Texture)>>>);

/// A compare's readback, mapped once its commands have long been
/// submitted (`AGE` flushes on), then reported to the arm.
struct Pending {
    buffer: wgpu::Buffer,
    lights: usize,
    age: u32,
    /// 0 waiting, 1 mapping, 2 mapped, 3 failed.
    state: Arc<AtomicU8>,
}

const AGE: u32 = 8;

static PENDING: Mutex<Vec<Pending>> = Mutex::new(Vec::new());

/// The binning and compare pipelines, per device.
static GPU: Mutex<Vec<(wgpu::Device, LightTiles, Compare)>> = Mutex::new(Vec::new());

fn with_gpu<T>(device: &wgpu::Device, f: impl FnOnce(&mut LightTiles, &Compare) -> T) -> T {
    let mut gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    if !gpu.iter().any(|(d, _, _)| d == device) {
        gpu.push((
            device.clone(),
            LightTiles::new(device),
            Compare::new(device),
        ));
    }
    let (_, tiles, compare) = gpu
        .iter_mut()
        .find(|(d, _, _)| d == device)
        .expect("just made");
    f(tiles, compare)
}

/// Maps the readbacks old enough and reports the mapped ones.
fn poll_compares() {
    let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    pending.retain_mut(|p| {
        p.age += 1;
        match p.state.load(Ordering::Acquire) {
            0 if p.age >= AGE => {
                p.state.store(1, Ordering::Release);
                let state = Arc::clone(&p.state);
                p.buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                    state.store(if r.is_ok() { 2 } else { 3 }, Ordering::Release);
                });
                true
            }
            2 => {
                let difference = match p.buffer.slice(..).get_mapped_range() {
                    Ok(bytes) => light_tiles_gpu::difference(&bytes),
                    Err(_) => f32::INFINITY,
                };
                p.buffer.unmap();
                ARM.report(difference <= light_tiles::tolerance(p.lights));
                false
            }
            3 => false,
            _ => true,
        }
    });
}

/// A map retired: the held run, pending compares and the arm start again.
pub(super) fn retire(run: &Mutex<Run>) {
    *run.lock().unwrap_or_else(|e| e.into_inner()) = Run::default();
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).clear();
    ARM.reset();
}

fn refused(cause: TileRefusal) {
    light_tiles::count(&[Segment::PerLight {
        light: 0,
        refused: Some(cause),
    }]);
}

impl<B: d3d::backend::Backend + Send + 'static> Glue<B> {
    /// Holds `draw`'s light in the run when the tiled path takes it.
    pub(super) fn hold_light(&self, mem: &mut dyn GuestMemory, draw: &SceneDraw<'_>) -> bool {
        if !ARM.on() {
            return false;
        }
        let full = {
            let run = self.run.lock().unwrap_or_else(|e| e.into_inner());
            run.view_target.is_some_and(|t| t != draw.view_target) || run.lights.len() >= MAX_RUN
        };
        if full {
            self.flush_run(mem);
        }
        match self.admit(&*mem, draw) {
            Ok(held) => {
                let mut run = self.run.lock().unwrap_or_else(|e| e.into_inner());
                run.view_target = Some(draw.view_target);
                run.lights.push(held);
                true
            }
            Err(Some(cause)) => {
                refused(cause);
                false
            }
            Err(None) => false,
        }
    }

    /// The light's layers 12 and 13 prepared as its per-light pass
    /// prepares them, if the tiled path takes them; `Err(None)` when the
    /// per-light pass would not take the light either.
    fn admit(
        &self,
        mem: &dyn GuestMemory,
        draw: &SceneDraw<'_>,
    ) -> Result<Held, Option<TileRefusal>> {
        let first = draw.passes.first().ok_or(None)?;
        let steps: Vec<u8> = draw.passes.iter().map(|p| p.group.group.layer).collect();
        let whole = TileVolume {
            rect: [0.0; 4],
            near: 0.0,
        };
        light_tiles_gpu::admit_light(&steps, &[], Some(whole)).map_err(Some)?;
        let (light, gel) = scene_light(&first.light).ok_or(None)?;
        if gel.is_some() {
            return Err(Some(TileRefusal::GelTarget));
        }
        let c = constants(&light).map_err(|_| None)?;
        let d = self.device.lock().map_err(|_| None)?;
        let ready = self.ready(mem).map_err(|_| None)?;
        let [_, _, x, y, w, h, _, _] = draw.view_target;
        let mut lit = Vec::new();
        for pass in draw.passes {
            let layer = pass.group.group.layer;
            if !matches!(layer, 12 | 13) || pass.group.entries.is_empty() {
                continue;
            }
            let draws = self
                .scene_light_draws(&d, mem, &ready, &pass.group, &c, false)
                .map_err(|_| None)?;
            let prepared = self
                .prepare_planned(
                    &d,
                    mem,
                    Arc::clone(&ready),
                    c,
                    draws,
                    None,
                    false,
                    Some([x, y, w, h]),
                )
                .map_err(|_| None)?;
            if prepared.projection.is_some()
                || prepared.shadow_map.is_some()
                || prepared.frozen.is_some()
            {
                return Err(Some(TileRefusal::Shadow));
            }
            for planned in &prepared.draws {
                light_tiles_gpu::admit_pass(&LitPass {
                    program: &planned.compiled.program,
                    gel_target: false,
                })
                .map_err(Some)?;
            }
            lit.push((layer, prepared));
        }
        Ok(Held {
            passes: draw.passes.to_vec(),
            lit,
        })
    }

    /// Draws the held run: tiled, compared, or per light when it does not
    /// prepare. Returns the lights drawn.
    pub(super) fn flush_run(&self, mem: &mut dyn GuestMemory) -> usize {
        poll_compares();
        let run = std::mem::take(&mut *self.run.lock().unwrap_or_else(|e| e.into_inner()));
        let Some(view_target) = run.view_target else {
            return 0;
        };
        let n = run.lights.len();
        if n == 0 {
            return 0;
        }
        if n > 1 {
            let decision = ARM.decide();
            if decision != Decision::Off {
                match self.draw_run(mem, view_target, &run.lights, decision) {
                    Ok(()) => {
                        light_tiles::count(&[Segment::Tiled(0..n)]);
                        return n;
                    }
                    Err(why) => {
                        light_tiles::fell_back();
                        let mut t = self.tally.lock().unwrap_or_else(|e| e.into_inner());
                        for msg in t.count(Err(why)) {
                            eprintln!("{msg}");
                        }
                    }
                }
            }
        } else {
            light_tiles::count(&[Segment::PerLight {
                light: 0,
                refused: None,
            }]);
        }
        let mut drawn = 0;
        for held in &run.lights {
            match self.scene_light_passes(mem, &held.passes, view_target) {
                Ok(_) => drawn += 1,
                Err(why) => {
                    light_tiles::lost();
                    let mut t = self.tally.lock().unwrap_or_else(|e| e.into_inner());
                    for msg in t.count(Err(why)) {
                        eprintln!("{msg}");
                    }
                }
            }
        }
        drawn
    }

    /// The run as one sequence: the tiled step alone, or for a compared
    /// run the tiled step into each copy's scratch, every light's
    /// per-light steps, and the compare.
    fn draw_run(
        &self,
        mem: &mut dyn GuestMemory,
        view_target: [u32; 8],
        lights: &[Held],
        decision: Decision,
    ) -> Result<(), Keep> {
        let mut d = self
            .device
            .lock()
            .map_err(|_| Keep::Unsupported("device unavailable"))?;
        if !d
            .begin_external_boundary(&mut DeviceMemory(mem))
            .map_err(|_| Keep::Unsupported("could not drain light state"))?
        {
            return Err(Keep::Unsupported("light has an outstanding push callback"));
        }
        if !d.can_external_pass() || d.bound_target() != view_target {
            return Err(Keep::Unsupported("light view target changed"));
        }
        let ready = self.ready(&*mem)?;
        if lights
            .iter()
            .flat_map(|h| &h.lit)
            .any(|(_, p)| !Arc::ptr_eq(&p.ready, &ready))
        {
            return Err(Keep::Unsupported("world changed during a held light run"));
        }
        let lit: Vec<Vec<(u8, PreparedLight, RetainedLight)>> = lights
            .iter()
            .map(|h| {
                h.lit
                    .iter()
                    .map(|(layer, p)| Ok((*layer, p.clone(), RetainedLight::of(&ready, p)?)))
                    .collect::<Result<Vec<_>, Keep>>()
            })
            .collect::<Result<_, _>>()?;
        let ranges: usize = lit
            .iter()
            .flatten()
            .flat_map(|(_, p, _)| &p.draws)
            .map(|p| p.ranges.len())
            .sum();
        if ranges == 0 {
            return Err(Keep::Unsupported("a held run draws nothing"));
        }
        let view = mem
            .u32(engine::render_loop::occlusion::guest::VIEW_INDEX)
            .unwrap_or(u32::MAX);
        let [color, depth, x, y, w, h, _, _] = view_target;
        let targets = || d3d::ExternalTargets {
            color,
            depth,
            rect: (x, y, w, h),
        };
        let plan = |ranges| d3d::ExternalPlan {
            stage: timeline::native::Stage::Lights,
            view,
            ranges,
            slots: Vec::new(),
        };
        let scratch = (decision == Decision::Compare).then(Scratch::default);
        let mut steps = vec![d3d::ExternalStep {
            targets: targets(),
            plan: plan(ranges),
            pass: Box::new(TiledRun {
                lit,
                ranges,
                view: None,
                clip: None,
                scratch: scratch.clone(),
            }),
        }];
        if let Some(scratch) = scratch {
            for held in lights {
                let sequence = self.light_sequence(&*mem, &held.passes)?;
                let (light_steps, _) = self.sequence_steps(
                    &d,
                    &*mem,
                    view_target,
                    &ready,
                    sequence,
                    timeline::native::Stage::Lights,
                )?;
                steps.extend(light_steps);
            }
            steps.push(d3d::ExternalStep {
                targets: targets(),
                plan: plan(1),
                pass: Box::new(CompareRun {
                    scratch,
                    lights: lights.len(),
                }),
            });
        }
        self.commit_steps(&mut d, mem, &ready, steps)
    }
}

/// The run's binning and instanced layers 12 and 13 on the view's
/// target, or, compared, on a scratch copy of it.
struct TiledRun {
    /// Per light, in the run's order (its bit), each lit layer prepared
    /// and retained.
    lit: Vec<Vec<(u8, PreparedLight, RetainedLight)>>,
    ranges: usize,
    view: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    clip: Option<d3d::DepthClip>,
    scratch: Option<Scratch>,
}

fn same_draw(
    a: &native_render::light_gpu::LightDraw<'_>,
    b: &native_render::light_gpu::LightDraw<'_>,
) -> bool {
    std::ptr::eq(a.gpu, b.gpu)
        && a.ranges == b.ranges
        && a.cull == b.cull
        && a.gel == b.gel
        && a.placement == b.placement
        && match (&a.model, &b.model) {
            (None, None) => true,
            (Some(x), Some(y)) => {
                x.vertices == y.vertices && x.indices == y.indices && x.layout.id == y.layout.id
            }
            _ => false,
        }
}

impl TiledRun {
    fn encoded(
        &mut self,
        t: &d3d::backend::ExternalAttachments,
        request: d3d::ExternalPassRequest,
    ) -> Result<d3d::backend::ExternalEncoded, &'static str> {
        let guest = request.surface_size;
        let constants = self.view.ok_or("missing view constants")?;
        let clip = self
            .clip
            .filter(|c| c.w_buffered == Some(true))
            .ok_or("unknown W-buffer at issue")?;
        let view = ViewInputs {
            constants: request.view_constants(t.copy, constants),
            guest_size: guest,
            size: t.size,
            z_scale: request.z_scale,
            w_clip: Some((clip.range, clip.clamp)),
        };
        let mut scissor = None;
        for (_, p, _) in self.lit.iter().flatten() {
            let [x, y, width, height] = p.scissor;
            let c = d3d::PixelRect {
                x,
                y,
                width,
                height,
            }
            .projected_x(request.rect, request.projection_factor(t.copy));
            let s = scale_scissor([c.x, c.y, c.width, c.height], guest, t.size)
                .ok_or("unsupported scissor scale")?;
            if scissor.is_some_and(|o| o != s) {
                return Err("held lights' scissors differ");
            }
            scissor = Some(s);
        }
        let scissor = scissor.ok_or("a held run draws nothing")?;
        let first = &self
            .lit
            .iter()
            .flatten()
            .next()
            .ok_or("a held run draws nothing")?
            .2;
        let mut groups: Vec<TiledDraw<'_>> = Vec::new();
        for (bit, light) in self.lit.iter().enumerate() {
            for (layer, p, held) in light {
                if held.vertices != first.vertices || held.indices != first.indices {
                    return Err("held lights draw over different world buffers");
                }
                for (k, planned) in p.draws.iter().enumerate() {
                    let mut draw = p.draw(planned);
                    draw.gpu = &held.materials[k];
                    if let Some(m) = &planned.model {
                        draw.model = Some(
                            m.geometry(&t.device)
                                .ok_or("model light part unavailable")?,
                        );
                    }
                    match groups
                        .iter_mut()
                        .find(|g| g.layer == *layer && same_draw(&g.lights[0].1, &draw))
                    {
                        Some(g) => g.lights.push((bit as u32, draw)),
                        None => groups.push(TiledDraw {
                            layer: *layer,
                            set: Some(&held.set),
                            lights: vec![(bit as u32, draw)],
                        }),
                    }
                }
            }
        }
        let depth_texture = t.depth.texture();
        if !depth_texture
            .usage()
            .contains(wgpu::TextureUsages::TEXTURE_BINDING)
        {
            return Err("the scene depth is not sampleable");
        }
        let depth_sample = depth_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("light tiles scene depth"),
            aspect: wgpu::TextureAspect::DepthOnly,
            dimension: Some(wgpu::TextureViewDimension::D2),
            base_mip_level: 0,
            mip_level_count: Some(1),
            base_array_layer: 0,
            array_layer_count: Some(1),
            ..Default::default()
        });
        let whole = TileVolume {
            rect: [0.0, 0.0, t.size.0 as f32, t.size.1 as f32],
            near: 0.0,
        };
        let volumes = vec![whole; self.lit.len()];
        let colour = t.color.texture().clone();
        let scratch = match &self.scratch {
            Some(shared) => {
                if !colour.usage().contains(wgpu::TextureUsages::COPY_SRC)
                    || (colour.width(), colour.height()) != t.size
                {
                    return Err("the view's target cannot be copied for the compare");
                }
                let texture = t.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("light tiles compare copy"),
                    size: colour.size(),
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: colour.format(),
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::COPY_DST
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[t.color_format],
                });
                let mut held = shared.0.lock().unwrap_or_else(|e| e.into_inner());
                held.retain(|(copy, _)| *copy != t.copy);
                held.push((t.copy, texture.clone()));
                Some(texture)
            }
            None => None,
        };
        let (grid, tiled) = with_gpu(&t.device, |tiles, _| {
            let grid = tiles.prepare_bin(&t.device, &depth_sample, t.size, &volumes);
            let tiled = tiles.record(
                &t.device,
                &first.set,
                &grid,
                &view,
                t.color_format,
                t.depth_format,
                &groups,
            );
            tiled.map(|tiled| (grid, tiled))
        })?;
        let draws = tiled.draws();
        let target_view = match &scratch {
            Some(s) => s.create_view(&wgpu::TextureViewDescriptor {
                format: Some(t.color_format),
                ..Default::default()
            }),
            None => t.color.clone(),
        };
        let depth_view = t.depth.clone();
        let stencil = t.depth_format.has_stencil_aspect();
        let (vertices, indices) = (first.vertices.clone(), first.indices.clone());
        Ok(d3d::backend::ExternalEncoded {
            ranges: self.ranges,
            draws,
            passes: 1,
            commands: Box::new(move |encoder| {
                if let Some(s) = &scratch {
                    encoder.copy_texture_to_texture(
                        colour.as_image_copy(),
                        s.as_image_copy(),
                        colour.size(),
                    );
                }
                grid.encode(encoder);
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("native light tiles"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: stencil.then(|| wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        }),
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                tiled.encode(&mut pass, &vertices, &indices, scissor);
            }),
        })
    }
}

impl d3d::ExternalPass for TiledRun {
    fn uses_validated_resources(&self) -> bool {
        true
    }

    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites {
            color: self.scratch.is_none(),
            depth_stencil: false,
        }
    }

    fn record_encoded(
        &mut self,
        t: &d3d::backend::ExternalAttachments,
        request: d3d::ExternalPassRequest,
    ) -> Option<Result<d3d::backend::ExternalEncoded, &'static str>> {
        Some(self.encoded(t, request))
    }

    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.view = Some(*constants);
    }

    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.clip = Some(clip);
    }
}

/// A compared run's last step: the per-light passes' result on the
/// view's target against the tiled pass's on the scratch copy, the
/// largest difference read back later ([`poll_compares`]).
struct CompareRun {
    scratch: Scratch,
    lights: usize,
}

impl d3d::ExternalPass for CompareRun {
    fn uses_validated_resources(&self) -> bool {
        true
    }

    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites::default()
    }

    fn record_encoded(
        &mut self,
        t: &d3d::backend::ExternalAttachments,
        _request: d3d::ExternalPassRequest,
    ) -> Option<Result<d3d::backend::ExternalEncoded, &'static str>> {
        let scratch = {
            let held = self.scratch.0.lock().unwrap_or_else(|e| e.into_inner());
            held.iter()
                .find(|(copy, _)| *copy == t.copy)
                .map(|(_, s)| s.clone())
        };
        let Some(scratch) = scratch else {
            return Some(Err("no tiled copy on this target copy"));
        };
        let colour = t.color.texture();
        if !colour
            .usage()
            .contains(wgpu::TextureUsages::TEXTURE_BINDING)
        {
            return Some(Err("the view's target cannot be read for the compare"));
        }
        let diff = with_gpu(&t.device, |_, compare| {
            compare.prepare(
                &t.device,
                &scratch.create_view(&Default::default()),
                &colour.create_view(&Default::default()),
                t.size,
            )
        });
        PENDING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Pending {
                buffer: diff.readback().clone(),
                lights: self.lights,
                age: 0,
                state: Arc::new(AtomicU8::new(0)),
            });
        Some(Ok(d3d::backend::ExternalEncoded {
            ranges: 1,
            draws: 0,
            passes: 0,
            commands: Box::new(move |encoder| diff.encode(encoder)),
        }))
    }
}
