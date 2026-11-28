//! The held run of slot 4's lights, drawn tiled (docs/graphics/
//! native-light-tiles.md, "In the game"): a light the tiled path admits is
//! held at its passes ([`Glue::hold_light`]); the run draws when the
//! next light draws on its own or the light frame ends
//! ([`Glue::flush_run`]), as one sequence whose step bins the run's
//! lights and draws their layers 12 and 13 instanced ([`TiledRun`]). A
//! run of one light draws per light.
//!
//! A shadowed light's layers 10 and 11 draw ahead of the run, in the
//! run's order, on the view's target; after each, the target's alpha (its
//! receivers' marks) is copied into a layer of the run's masks
//! ([`Snapshot`]), layer 0 the alpha the run began with. A lit pass whose
//! blend reads the destination's alpha reads its light's layer instead
//! (`light_tiles_gpu::LightTiles::record_masked`). No lit pass of a
//! masked run writes alpha, so each layer holds the alpha that light's
//! passes read per light.

use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use native_render::light_tiles::{self, MAX_RUN, Segment, TileRefusal, TileVolume};
use native_render::light_tiles_gpu::{self, AlphaRead, LightTiles, LitPass, MAX_SHADOWED, TiledDraw};
use native_render::program::ViewInputs;
use native_render::stage::{Stage, StageWork};
use render_bridge::native_lights::{SceneDraw, ScenePass};

use super::{
    Glue, Keep, PreparedLight, RetainedLight, ShadowMap, constants, intersect_scissor, scale_scissor,
    scene_light, structure_key, view_constants, w_clip,
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

/// A run's masks ([`Snapshot`]): a layer per shadowed light after the
/// first, the target's format and size.
#[derive(Clone, Default)]
struct Masks(Arc<Mutex<Option<wgpu::Texture>>>);

impl Masks {
    fn get(&self) -> Option<wgpu::Texture> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// The binning pipelines, per device.
static GPU: Mutex<Vec<(wgpu::Device, LightTiles)>> = Mutex::new(Vec::new());

fn with_gpu<T>(device: &wgpu::Device, f: impl FnOnce(&mut LightTiles) -> T) -> T {
    let mut gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    if !gpu.iter().any(|(d, _)| d == device) {
        gpu.push((device.clone(), LightTiles::new(device)));
    }
    let (_, tiles) = gpu.iter_mut().find(|(d, _)| d == device).expect("just made");
    f(tiles)
}

/// A map retired: the held run starts again.
pub(super) fn retire(run: &Mutex<Run>) {
    *run.lock().unwrap_or_else(|e| e.into_inner()) = Run::default();
}

fn refused(cause: TileRefusal) {
    light_tiles::count(&[Segment::PerLight {
        light: 0,
        refused: Some(cause),
    }]);
}

impl Glue {
    /// Holds `draw`'s light in the run when the tiled path takes it.
    pub(super) fn hold_light(&self, mem: &mut dyn GuestMemory, draw: &SceneDraw<'_>) -> bool {
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
                .scene_light_draws(mem, &ready, &pass.group, &c, false)
                .map_err(|_| None)?;
            let prepared = self
                .prepare_planned(mem, Arc::clone(&ready), c, draws, None, Some([x, y, w, h]))
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

    /// Draws the held run: tiled, or per light for a run of one. Returns
    /// the lights drawn.
    pub(super) fn flush_run(&self, mem: &mut dyn GuestMemory) -> usize {
        let run = std::mem::take(&mut *self.run.lock().unwrap_or_else(|e| e.into_inner()));
        let Some(view_target) = run.view_target else {
            return 0;
        };
        let n = run.lights.len();
        if n == 0 {
            return 0;
        }
        if n > 1 {
            light_tiles::count(&[Segment::Tiled(0..n)]);
            return super::scene_drawn(self.draw_run(&*mem, &run.lights)).map_or(0, |()| n);
        }
        light_tiles::count(&[Segment::PerLight {
            light: 0,
            refused: None,
        }]);
        let mut drawn = 0;
        for held in &run.lights {
            if super::scene_drawn(self.scene_light_passes(mem, &held.passes, view_target)).is_some() {
                drawn += 1;
            }
        }
        drawn
    }

    /// The run as one sequence on the view's target: its shadows and
    /// masks, then the tiled step.
    fn draw_run(&self, mem: &dyn GuestMemory, lights: &[Held]) -> Result<(), Keep> {
        let ready = self.ready(mem)?;
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
        let targets = (frame_types::TargetRef::SceneColour, frame_types::TargetRef::SceneDepth);
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
        let snapshot = |layer: u32| super::scene::Step {
            targets,
            stage: Box::new(Snapshot {
                masks: masks.clone().unwrap_or_default(),
                layer,
                layers: shadowed + 1,
            }),
        };
        // A shadowed light's shadow steps from its own sequence.
        let shadow_steps = |held: &Held| -> Result<Vec<super::scene::Step>, Keep> {
            let sequence = self.light_sequence(mem, &held.passes)?.part(false);
            Ok(self.sequence_steps(mem, &ready, sequence)?.0)
        };
        let mut steps = Vec::new();
        if masked {
            steps.push(snapshot(0));
        }
        // The shadows ahead of the run, each followed by its mask layer.
        let mut layer = 0;
        for held in lights.iter().filter(|h| h.shadowed) {
            layer += 1;
            steps.extend(shadow_steps(held)?);
            steps.push(snapshot(layer));
        }
        steps.push(super::scene::Step {
            targets,
            stage: Box::new(TiledRun {
                lit,
                bounds: lights.iter().map(|h| h.bounds).collect(),
                ranges,
                masks,
                alpha_layers,
            }),
        });
        self.commit_steps(mem, &ready, frame_types::Slot::Opaque, steps)
    }
}

/// The run's binning and instanced layers 12 and 13 on the view's
/// target.
struct TiledRun {
    /// Per light, in the run's order (its bit), each lit layer prepared
    /// and retained.
    lit: Vec<Vec<(u8, PreparedLight, RetainedLight)>>,
    /// Per light, its box ([`Held::bounds`]).
    bounds: Vec<Option<Bounds>>,
    ranges: usize,
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
        t: &gfx::external::ExternalAttachments,
        request: &native_render::program::ViewInputs,
    ) -> Result<gfx::external::ExternalEncoded, &'static str> {
        let guest = request.guest_size;
        let view = ViewInputs { size: t.size, ..*request };
        let mut scissor = None;
        for (_, p, _) in self.lit.iter().flatten() {
            let [x, y, width, height] = intersect_scissor(p.scissor, crate::native_world_glue::view_rect(request));
            let c = gfx::external::PixelRect {
                x,
                y,
                width,
                height,
            }
            .projected_x(crate::native_world_glue::view_rect(request), crate::native_world_glue::projection_factor());
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
        // Each light's box through the view (`TileVolume::from_bounds`);
        // a light with no box covers the whole target from depth 0.
        let volumes: Vec<TileVolume> = self
            .bounds
            .iter()
            .map(|b| {
                b.and_then(|b| TileVolume::from_bounds(&view, b))
                    .unwrap_or(whole)
            })
            .collect();
        let masks_view = match &self.masks {
            Some(masks) => Some(
                masks
                    .get()
                    .ok_or("no light masks")?
                    .create_view(&light_tiles_gpu::mask_view()),
            ),
            None => None,
        };
        let (grid, tiled) = with_gpu(&t.device, |tiles| {
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
        let target_view = t.color.clone();
        let depth_view = t.depth.clone();
        let stencil = t.depth_format.has_stencil_aspect();
        let (vertices, indices) = (first.vertices.clone(), first.indices.clone());
        Ok(gfx::external::ExternalEncoded {
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

impl Stage for TiledRun {
    fn name(&self) -> &'static str {
        "light tiles"
    }

    fn record(&mut self, t: &gfx::external::ExternalAttachments, view: &native_render::program::ViewInputs) -> StageWork {
        match self.encoded(t, view) {
            Ok(encoded) => StageWork::Encoded(encoded),
            Err(why) => unimplemented!("lights: tiles: {why}"),
        }
    }
}

/// The view's target's alpha now, copied into layer `layer` of the run's
/// masks (the whole texel, the format the passes read); the layer 0
/// step makes the `layers`-layer array.
struct Snapshot {
    masks: Masks,
    layer: u32,
    layers: u32,
}

impl Stage for Snapshot {
    fn name(&self) -> &'static str {
        "light tiles mask"
    }

    fn record(&mut self, t: &gfx::external::ExternalAttachments, _: &native_render::program::ViewInputs) -> StageWork {
        let colour = t.color.texture().clone();
        if !colour.usage().contains(wgpu::TextureUsages::COPY_SRC)
            || (colour.width(), colour.height()) != t.size
            || colour.sample_count() != 1
        {
            unimplemented!("lights: the view's target cannot be copied into the light masks");
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
            *self.masks.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(texture.clone());
            texture
        } else {
            match self.masks.get() {
                Some(texture) => texture,
                None => unimplemented!("lights: no light masks before layer {}", self.layer),
            }
        };
        if self.layer >= masks.depth_or_array_layers() {
            unimplemented!("lights: a light mask layer past the run's masks");
        }
        let (layer, size) = (self.layer, t.size);
        StageWork::Encoded(gfx::external::ExternalEncoded {
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
        })
    }
}
