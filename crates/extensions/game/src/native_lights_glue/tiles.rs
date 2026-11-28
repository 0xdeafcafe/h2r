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
//!
//! A shadowed light's layers 10 and 11 draw ahead of the run, in the
//! run's order, on the view's target; after each, the target's alpha (its
//! receivers' marks) is copied into a layer of the run's masks
//! ([`Snapshot`]), layer 0 the alpha the run began with. A lit pass whose
//! blend reads the destination's alpha reads its light's layer instead
//! (`light_tiles_gpu::LightTiles::record_masked`). No lit pass of a
//! masked run writes alpha, so each layer holds the alpha that light's
//! passes read per light.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use native_render::light_tiles::{self, ARM, Decision, MAX_RUN, Segment, TileRefusal, TileVolume};
use native_render::light_tiles_gpu::{
    self, AlphaRead, Compare, LightTiles, LitPass, MAX_SHADOWED, TiledDraw,
};
use native_render::program::ViewInputs;
use render_bridge::native_lights::{SceneDraw, ScenePass};

use super::{
    DeviceMemory, Glue, Keep, PreparedLight, RetainedLight, ShadowMap, constants, scale_scissor,
    scene_light, structure_key,
};

/// A world-space box (`min`, `max`).
type Bounds = ([f32; 3], [f32; 3]);

/// One held light: its passes as the scene gave them, its layers 12
/// and 13 prepared at its passes, and the world-space box of what they
/// draw; `None` when a part's box is not resident (an object's), and the
/// light then covers the whole target.
pub(super) struct Held {
    passes: Vec<ScenePass>,
    lit: Vec<(u8, PreparedLight)>,
    bounds: Option<Bounds>,
    /// Its layers 10 and 11 draw ahead of the run, a mask layer after.
    shadowed: bool,
    /// A lit pass's blend reads the destination's alpha.
    reads: bool,
    /// A lit pass writes alpha.
    writes_alpha: bool,
}

impl Held {
    /// The run needs masks for this light.
    fn masks(&self) -> bool {
        self.shadowed || self.reads
    }
}

/// The box of `entries`' structure parts (`WorldDraw::bounds`, world
/// space for placements too); `None` for an object entry or a part that
/// is not resident.
fn entry_bounds<'a>(
    world: &super::LightsWorld,
    entries: impl IntoIterator<Item = &'a frame_scene::LightEntry>,
    into: &mut native_render::world::Aabb,
) -> Option<()> {
    for e in entries {
        structure_key(e)?;
        for (draw, _) in world.structure_entry(e).ok()? {
            into.add(draw.bounds.min);
            into.add(draw.bounds.max);
        }
    }
    Some(())
}

/// The run of held lights of one view's target.
#[derive(Default)]
pub(super) struct Run {
    view_target: Option<[u32; 8]>,
    lights: Vec<Held>,
}

impl Run {
    /// `h` cannot join the run as it is: a masked run takes no light that
    /// writes alpha (the masks would miss its add), a run with such a
    /// light no masked one, and at most [`MAX_SHADOWED`] shadowed lights.
    fn refuses(&self, h: &Held) -> bool {
        let masks = self.lights.iter().any(Held::masks);
        let writes = self.lights.iter().any(|l| l.writes_alpha);
        let shadowed = self.lights.iter().filter(|l| l.shadowed).count();
        (masks && h.writes_alpha)
            || (writes && h.masks())
            || (h.shadowed && shadowed >= MAX_SHADOWED)
    }
}

/// Each copy's masks of a run ([`Snapshot`]): a layer per shadowed light
/// after the first, the target's format and size.
#[derive(Clone, Default)]
struct Masks(Arc<Mutex<Vec<(d3d::TargetCopy, wgpu::Texture)>>>);

impl Masks {
    fn on(&self, copy: d3d::TargetCopy) -> Option<wgpu::Texture> {
        let held = self.0.lock().unwrap_or_else(|e| e.into_inner());
        held.iter()
            .find(|(c, _)| *c == copy)
            .map(|(_, t)| t.clone())
    }
}

/// Each copy's scratch target a compared run draws its tiled pass into.
#[derive(Clone, Default)]
struct Scratch(Arc<Mutex<Vec<(d3d::TargetCopy, wgpu::Texture)>>>);

impl Scratch {
    fn on(&self, copy: d3d::TargetCopy) -> Option<wgpu::Texture> {
        let held = self.0.lock().unwrap_or_else(|e| e.into_inner());
        held.iter()
            .find(|(c, _)| *c == copy)
            .map(|(_, t)| t.clone())
    }
}

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
        match self.admit(&*mem, draw) {
            Ok(held) => {
                let full = {
                    let run = self.run.lock().unwrap_or_else(|e| e.into_inner());
                    run.view_target.is_some_and(|t| t != draw.view_target)
                        || run.lights.len() >= MAX_RUN
                        || run.refuses(&held)
                };
                if full {
                    self.flush_run(mem);
                }
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
        let shadowed = steps.iter().any(|&l| l == 10 || l == 11);
        if !shadowed {
            light_tiles_gpu::admit_light(&steps, &[], Some(whole)).map_err(Some)?;
        }
        let (light, gel) = scene_light(&first.light).ok_or(None)?;
        if gel.is_some() {
            return Err(Some(TileRefusal::GelTarget));
        }
        let c = constants(&light).map_err(|_| None)?;
        let d = self.device.lock().map_err(|_| None)?;
        let ready = self.ready(mem).map_err(|_| None)?;
        let [_, _, x, y, w, h, _, _] = draw.view_target;
        let world = ready.lights.get().ok_or(None)?;
        if shadowed {
            // Its casters and receivers draw ahead of the run as its own
            // sequence would draw them; the receivers must mark alpha only.
            self.light_sequence(mem, draw.passes).map_err(|_| None)?;
            let view = mem
                .u32(engine::render_loop::occlusion::guest::VIEW_INDEX)
                .unwrap_or(u32::MAX);
            let mut receivers = Vec::new();
            for pass in draw.passes {
                if pass.group.group.layer != 11 || pass.group.entries.is_empty() {
                    continue;
                }
                let prepared = self
                    .shadow_prepared(
                        &d,
                        mem,
                        view,
                        &pass.group,
                        Some(c),
                        ShadowMap::Frozen(Some(super::scene::Frozen::default())),
                        Some([x, y, w, h]),
                        None,
                    )
                    .map_err(|_| None)?;
                receivers.extend(
                    prepared
                        .draws
                        .iter()
                        .map(|p| p.compiled.program.state.color_writes),
                );
            }
            light_tiles_gpu::admit_shadowed_light(&steps, &receivers).map_err(Some)?;
        }
        let (mut reads, mut writes_alpha) = (false, false);
        let mut lit = Vec::new();
        let mut aabb = native_render::world::Aabb::EMPTY;
        let mut bounded = true;
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
                let program = &planned.compiled.program;
                light_tiles_gpu::admit_pass(&LitPass {
                    program,
                    gel_target: false,
                })
                .map_err(Some)?;
                reads |= light_tiles_gpu::alpha_read(program) != AlphaRead::None;
                writes_alpha |= program
                    .state
                    .color_writes
                    .contains(wgpu::ColorWrites::ALPHA);
            }
            bounded &= entry_bounds(world, &pass.group.entries, &mut aabb).is_some();
            lit.push((layer, prepared));
        }
        Ok(Held {
            passes: draw.passes.to_vec(),
            lit,
            bounds: (bounded && !aabb.is_empty()).then_some((aabb.min, aabb.max)),
            shadowed,
            reads,
            writes_alpha,
        })
        .and_then(|h| {
            // Its own alpha add would land after the later receivers'
            // marks, which per light come after it.
            if h.shadowed && h.writes_alpha {
                Err(Some(TileRefusal::Shadow))
            } else {
                Ok(h)
            }
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
            if super::scene_drawn(self.scene_light_passes(mem, &held.passes, view_target)).is_some() {
                drawn += 1;
            }
        }
        drawn
    }

    /// The run as one sequence: its shadows and masks, then the tiled
    /// step; or for a compared run a scratch copy of the target, every
    /// light's per-light steps (the masks taken among them), the tiled
    /// step into the scratch copy, and the compare.
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
        // The masks: layer 0 the alpha the run began with, then one per
        // shadowed light after its receivers; each light reads the layer
        // of the last shadowed light at or before it.
        let masked = lights.iter().any(Held::masks);
        let mut alpha_layers = Vec::with_capacity(lights.len());
        let mut shadowed = 0u32;
        for h in lights {
            shadowed += u32::from(h.shadowed);
            alpha_layers.push(shadowed);
        }
        let masks = masked.then(Masks::default);
        let snapshot = |layer: u32| d3d::ExternalStep {
            targets: targets(),
            plan: plan(1),
            pass: Box::new(Snapshot {
                masks: masks.clone().unwrap_or_default(),
                layer,
                layers: shadowed + 1,
            }),
        };
        // One light's steps from its own sequence: all of them, or its
        // shadow's (`lit` false) or its layers 12 and 13 (`lit` true).
        let light_steps =
            |held: &Held, part: Option<bool>| -> Result<Vec<d3d::ExternalStep>, Keep> {
                let mut sequence = self.light_sequence(&*mem, &held.passes)?;
                if let Some(lit) = part {
                    sequence = sequence.part(lit);
                }
                let (steps, _) = self.sequence_steps(
                    &d,
                    &*mem,
                    view_target,
                    &ready,
                    sequence,
                    timeline::native::Stage::Lights,
                )?;
                Ok(steps)
            };
        let scratch = (decision == Decision::Compare).then(Scratch::default);
        let mut steps = Vec::new();
        if let Some(scratch) = &scratch {
            steps.push(d3d::ExternalStep {
                targets: targets(),
                plan: plan(1),
                pass: Box::new(ScratchCopy {
                    scratch: scratch.clone(),
                }),
            });
        }
        if masked {
            steps.push(snapshot(0));
        }
        // The shadows ahead of the run, each followed by its mask layer;
        // compared, every light's per-light steps on the target, with a
        // shadowed light's mask layer between its receivers and its
        // layers 12 and 13.
        let mut layer = 0;
        for held in lights {
            match (&scratch, held.shadowed) {
                (None, false) => {}
                (None, true) => {
                    layer += 1;
                    steps.extend(light_steps(held, Some(false))?);
                    steps.push(snapshot(layer));
                }
                (Some(_), false) => steps.extend(light_steps(held, None)?),
                (Some(_), true) => {
                    layer += 1;
                    steps.extend(light_steps(held, Some(false))?);
                    steps.push(snapshot(layer));
                    steps.extend(light_steps(held, Some(true))?);
                }
            }
        }
        steps.push(d3d::ExternalStep {
            targets: targets(),
            plan: plan(ranges),
            pass: Box::new(TiledRun {
                lit,
                bounds: lights.iter().map(|h| h.bounds).collect(),
                ranges,
                view: None,
                clip: None,
                scratch: scratch.clone(),
                masks,
                alpha_layers,
            }),
        });
        if let Some(scratch) = scratch {
            steps.push(d3d::ExternalStep {
                targets: targets(),
                plan: plan(1),
                pass: Box::new(CompareRun {
                    scratch,
                    lights: lights.len(),
                    alpha: !masked,
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
    /// Per light, its box ([`Held::bounds`]).
    bounds: Vec<Option<Bounds>>,
    ranges: usize,
    view: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    clip: Option<d3d::DepthClip>,
    scratch: Option<Scratch>,
    /// The run's masks and, per light, the layer its passes read.
    masks: Option<Masks>,
    alpha_layers: Vec<u32>,
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
        // Each light's box through the copy's view (`TileVolume::from_bounds`);
        // a light with no box covers the whole target from depth 0.
        let volumes: Vec<TileVolume> = self
            .bounds
            .iter()
            .map(|b| {
                b.and_then(|b| TileVolume::from_bounds(&view, b))
                    .unwrap_or(whole)
            })
            .collect();
        let scratch = match &self.scratch {
            Some(shared) => Some(
                shared
                    .on(t.copy)
                    .ok_or("no compare copy on this target copy")?,
            ),
            None => None,
        };
        let masks_view = match &self.masks {
            Some(masks) => Some(
                masks
                    .on(t.copy)
                    .ok_or("no light masks on this target copy")?
                    .create_view(&light_tiles_gpu::mask_view()),
            ),
            None => None,
        };
        let (grid, tiled) = with_gpu(&t.device, |tiles, _| {
            let grid = tiles.prepare_bin(&t.device, &depth_sample, t.size, &volumes);
            let tiled = tiles.record_masked(
                &t.device,
                &first.set,
                &grid,
                &view,
                t.color_format,
                t.depth_format,
                &groups,
                masks_view.as_ref().map(|v| (v, &self.alpha_layers[..])),
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
                        stencil_ops: stencil.then_some(wgpu::Operations {
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
    /// Compare alpha too: not for a masked run, whose tiled copy keeps the
    /// alpha it began with.
    alpha: bool,
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
        let Some(scratch) = self.scratch.on(t.copy) else {
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
            compare.prepare_channels(
                &t.device,
                &scratch.create_view(&Default::default()),
                &colour.create_view(&Default::default()),
                t.size,
                self.alpha,
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

/// A compared run's first step: each copy's scratch copy of the view's
/// target as the run begins, which the tiled step draws into.
struct ScratchCopy {
    scratch: Scratch,
}

impl d3d::ExternalPass for ScratchCopy {
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
        let colour = t.color.texture().clone();
        if !colour.usage().contains(wgpu::TextureUsages::COPY_SRC)
            || (colour.width(), colour.height()) != t.size
        {
            return Some(Err("the view's target cannot be copied for the compare"));
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
        {
            let mut held = self.scratch.0.lock().unwrap_or_else(|e| e.into_inner());
            held.retain(|(copy, _)| *copy != t.copy);
            held.push((t.copy, texture.clone()));
        }
        Some(Ok(d3d::backend::ExternalEncoded {
            ranges: 1,
            draws: 0,
            passes: 0,
            commands: Box::new(move |encoder| {
                encoder.copy_texture_to_texture(
                    colour.as_image_copy(),
                    texture.as_image_copy(),
                    colour.size(),
                );
            }),
        }))
    }
}

/// The view's target's alpha now, copied into layer `layer` of the run's
/// masks on each copy (the whole texel, the format the passes read); the
/// layer 0 step makes the copy's `layers`-layer array.
struct Snapshot {
    masks: Masks,
    layer: u32,
    layers: u32,
}

impl d3d::ExternalPass for Snapshot {
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
        let colour = t.color.texture().clone();
        if !colour.usage().contains(wgpu::TextureUsages::COPY_SRC)
            || (colour.width(), colour.height()) != t.size
            || colour.sample_count() != 1
        {
            return Some(Err(
                "the view's target cannot be copied into the light masks",
            ));
        }
        let masks = if self.layer == 0 {
            let texture = t.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("light tiles masks"),
                size: wgpu::Extent3d {
                    width: t.size.0,
                    height: t.size.1,
                    depth_or_array_layers: self.layers,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: colour.format(),
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let mut held = self.masks.0.lock().unwrap_or_else(|e| e.into_inner());
            held.retain(|(copy, _)| *copy != t.copy);
            held.push((t.copy, texture.clone()));
            texture
        } else {
            match self.masks.on(t.copy) {
                Some(texture) => texture,
                None => return Some(Err("no light masks on this target copy")),
            }
        };
        if self.layer >= masks.depth_or_array_layers() {
            return Some(Err("a light mask layer past the run's masks"));
        }
        let (layer, size) = (self.layer, t.size);
        Some(Ok(d3d::backend::ExternalEncoded {
            ranges: 1,
            draws: 0,
            passes: 0,
            commands: Box::new(move |encoder| {
                encoder.copy_texture_to_texture(
                    colour.as_image_copy(),
                    wgpu::TexelCopyTextureInfo {
                        texture: &masks,
                        mip_level: 0,
                        origin: wgpu::Origin3d {
                            x: 0,
                            y: 0,
                            z: layer,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width: size.0,
                        height: size.1,
                        depth_or_array_layers: 1,
                    },
                );
            }),
        }))
    }
}
