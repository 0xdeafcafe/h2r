//! A cut light's layers drawn from the scene (docs/graphics/native-lights.md,
//! "The light frame under the cut"). The guest's layer begin and end do
//! not run, so the work they make is the light's own steps, each a stage
//! pushed after a bind of its attachments:
//!
//! - layer 10: target 9's depth cleared to 1 and stencil 0 (the begin's
//!   `0x00014b60`), the casters drawn into it, and the region they drew
//!   frozen into a map ([`Frozen`],
//!   `gfx::depth_bits::DepthBits::convert_region`); a colour-mask caster
//!   the same on target 10, cleared white with zero alpha, its colour
//!   copied (an object shadow's filter runs between the casters and the
//!   copy, [`object_ops`]);
//! - layer 11: the receivers on the view's target, sampling that map;
//! - layer 14: target 1;
//! - layers 12 and 13: the view's target.
//!
//! The sequence is pushed whole, or not at all.

use super::*;
use frame_scene::ShadowPass;
use frame_types::TargetRef;
use native_render::shadow_gpu::{MapKind, SampledMap};
use render_bridge::native_lights::{SceneGroup, ScenePass};

/// Where a step draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Attach {
    /// The view's target the light pass ran on.
    View,
    /// Render target `n` (`engine::shadow_constants::global::TARGETS`).
    Target(u32),
}

/// One ordered operation of a light drawn from the scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Op {
    /// Clear a shadow map target: target 9's depth to 1 and stencil to 0,
    /// target 10's colour to white with zero alpha.
    Clear(u32),
    /// Draw the light's pass `n` (an index into its passes).
    Draw(usize, Attach),
    /// An object shadow's filter quads over targets 10 and 11.
    Filter,
    /// Freeze a shadow map target for the receivers after it: target 9's
    /// depth bits, target 10's colour.
    Freeze(u32),
    /// The light's shader gel into target 13's level 0, before the layers
    /// that sample it (`crate::native_gels_scene`).
    Gel,
}

/// The map target of a caster's pass: 9 for the depth map, 10 for the
/// colour mask (pass flag 4 set with 5 clear).
fn caster_target(pass: Option<ShadowPass>) -> Result<u32, Keep> {
    let pass = pass.ok_or(Keep::Shadow(native_render::shadow_scene::Refusal::NoPass.name()))?;
    match (pass.colour_mask(), pass.target) {
        (false, 9) => Ok(9),
        (true, 10) => Ok(10),
        _ => Err(Keep::Unsupported("a scene caster's target is not its map's")),
    }
}

/// A light's operations from its passes' layers and shadow states, in the
/// passes' order. A layer the passes do not draw refuses the light.
pub(super) fn ops(layers: &[(u8, Option<ShadowPass>)]) -> Result<Vec<Op>, Keep> {
    let mut out = Vec::with_capacity(layers.len() + 2);
    for (n, &(layer, pass)) in layers.iter().enumerate() {
        match layer {
            10 => {
                let t = caster_target(pass)?;
                out.extend([Op::Clear(t), Op::Draw(n, Attach::Target(t)), Op::Freeze(t)]);
            }
            11..=13 => out.push(Op::Draw(n, Attach::View)),
            14 => out.push(Op::Draw(n, Attach::Target(1))),
            _ => return Err(Keep::Unsupported("a scene light layer the passes do not draw")),
        }
    }
    Ok(out)
}

/// [`ops`] for a light with a shader gel: the gel into target 13 first,
/// as the constants' producer `0x0003a5ab` draws it before any layer.
pub(super) fn gel_ops(layers: &[(u8, Option<ShadowPass>)], gel: bool) -> Result<Vec<Op>, Keep> {
    let mut out = ops(layers)?;
    if gel {
        out.insert(0, Op::Gel);
    }
    Ok(out)
}

/// An object shadow's operations (`0x00032350`): its caster's layer 10
/// into the colour mask, the filter when it has one, the mask frozen, and
/// its layer 11 receivers on the view's target. Anything else refuses it.
pub(super) fn object_ops(layers: &[(u8, Option<ShadowPass>)], filter: bool) -> Result<Vec<Op>, Keep> {
    if !matches!(layers, [(10, _), (11, _)]) || caster_target(layers[0].1)? != 10 {
        return Err(Keep::Unsupported("an object shadow is not a mask caster and its receivers"));
    }
    let mut out = ops(layers)?;
    if filter {
        let at = out.iter().position(|op| *op == Op::Freeze(10)).ok_or(Keep::Shadow("state: an object shadow with no mask freeze"))?;
        out.insert(at, Op::Filter);
    }
    Ok(out)
}

/// Clears a shadow map (the layer begin's clear): target 9's depth and
/// stencil, or target 10's colour (`colour`).
struct ClearMap {
    colour: bool,
}

impl Stage for ClearMap {
    fn name(&self) -> &'static str {
        "light shadow clear"
    }

    fn record(&mut self, t: &gfx::external::ExternalAttachments, _: &native_render::program::ViewInputs) -> StageWork {
        let (colour, depth) = (self.colour, t.depth.clone());
        let stencil = t.depth_format.has_stencil_aspect();
        let color = t.color.clone();
        StageWork::Encoded(gfx::external::ExternalEncoded {
            ranges: 1,
            draws: 0,
            passes: 1,
            commands: Box::new(move |encoder| {
                fn clear<V>(v: V) -> wgpu::Operations<V> {
                    wgpu::Operations {
                        load: wgpu::LoadOp::Clear(v),
                        store: wgpu::StoreOp::Store,
                    }
                }
                let attachment = [Some(wgpu::RenderPassColorAttachment {
                    view: &color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: clear(MASK_CLEAR),
                })];
                drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("scene light shadow clear"),
                    color_attachments: if colour { &attachment } else { &[] },
                    depth_stencil_attachment: (!colour).then(|| wgpu::RenderPassDepthStencilAttachment {
                        view: &depth,
                        depth_ops: Some(clear(1.0)),
                        stencil_ops: stencil.then(|| clear(0)),
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                }));
            }),
        })
    }
}

/// The colour mask's clear: white, zero alpha (native-shadows.md).
const MASK_CLEAR: wgpu::Color = wgpu::Color {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 0.0,
};

/// The caster map a light froze, its freeze recorded before its
/// receivers, which sample it.
#[derive(Clone, Default)]
pub(super) struct Frozen(Arc<Mutex<Option<SampledMap>>>);

impl Frozen {
    fn put(&self, map: SampledMap) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(map);
    }

    /// The map last frozen.
    pub(super) fn get(&self) -> Option<SampledMap> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// The texels of a caster map: the caster's guest `rect` (from the
/// target's top-left) scaled from the target's guest size `surface` to
/// its `texels`. `None` for an empty or offset rect, or one past the
/// target.
pub(super) fn frozen_size(rect: (u32, u32, u32, u32), surface: (u32, u32), texels: (u32, u32)) -> Option<(u32, u32)> {
    let (x, y, w, h) = rect;
    let (gw, gh) = surface;
    if x != 0 || y != 0 || w == 0 || h == 0 || w > gw || h > gh {
        return None;
    }
    let [_, _, sw, sh] = gfx::raster::RenderScale::between(surface, texels)?.rect(x, y, w, h);
    (sw != 0 && sh != 0 && sw <= texels.0 && sh <= texels.1).then_some((sw, sh))
}

/// The depth-bits pipeline of `device`, made once per device.
fn depth_bits(device: &wgpu::Device) -> Arc<gfx::depth_bits::DepthBits> {
    static BITS: Mutex<Vec<(wgpu::Device, Arc<gfx::depth_bits::DepthBits>)>> = Mutex::new(Vec::new());
    let mut bits = BITS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, b)) = bits.iter().find(|(d, _)| d == device) {
        return Arc::clone(b);
    }
    let b = Arc::new(gfx::depth_bits::DepthBits::on_device(device));
    bits.push((device.clone(), Arc::clone(&b)));
    b
}

/// Freezes the region of a caster map its casters drew, at the target's
/// texels, for the receivers after it ([`Frozen`]): target 9's depth as
/// its bits, target 10's colour mask as a copy.
struct FreezeMap {
    frozen: Frozen,
    kind: MapKind,
}

impl Stage for FreezeMap {
    fn name(&self) -> &'static str {
        "light shadow freeze"
    }

    fn record(&mut self, t: &gfx::external::ExternalAttachments, request: &native_render::program::ViewInputs) -> StageWork {
        let source = match self.kind {
            MapKind::Depth => t.depth.texture().clone(),
            MapKind::ColourMask => t.color.texture().clone(),
        };
        let Some(size) = frozen_size(crate::native_world_glue::view_rect(request), request.guest_size, t.size)
            .filter(|&(w, h)| w <= source.width() && h <= source.height())
        else {
            unimplemented!("lights: a scene light caster map outside its target");
        };
        let usable = source.sample_count() == 1
            && match self.kind {
                MapKind::Depth => {
                    source.format() == wgpu::TextureFormat::Depth32FloatStencil8
                        && source.usage().contains(wgpu::TextureUsages::TEXTURE_BINDING)
                }
                MapKind::ColourMask => {
                    matches!(
                        source.format(),
                        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm
                    ) && source.usage().contains(wgpu::TextureUsages::COPY_SRC)
                }
            };
        if !usable {
            unimplemented!("lights: a scene light caster map that cannot be sampled");
        }
        let out = match self.kind {
            MapKind::Depth => gfx::depth_bits::DepthBits::output(&t.device, size),
            MapKind::ColourMask => t.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("scene frozen object shadow"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: source.format(),
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            }),
        };
        self.frozen.put(
            SampledMap {
                kind: self.kind,
                view: out.create_view(&Default::default()),
                size,
            },
        );
        let commands: Box<dyn Fn(&mut wgpu::CommandEncoder) + Send> = match self.kind {
            MapKind::Depth => {
                let (bits, device, queue) = (depth_bits(&t.device), t.device.clone(), t.queue.clone());
                Box::new(move |encoder| bits.convert_region(&device, &queue, encoder, &source, &out))
            }
            MapKind::ColourMask => Box::new(move |encoder| {
                encoder.copy_texture_to_texture(
                    source.as_image_copy(),
                    out.as_image_copy(),
                    out.size(),
                )
            }),
        };
        StageWork::Encoded(gfx::external::ExternalEncoded {
            ranges: 1,
            draws: 1,
            passes: 0,
            commands,
        })
    }
}

/// What a sequence draws from the scene: the groups its operations index,
/// the light's constants (none for an object shadow) and an object
/// shadow's filter.
pub(super) struct Sequence<'a> {
    groups: Vec<&'a SceneGroup>,
    light: Option<LightConstants>,
    filter: Option<frame_scene::ShadowFilter>,
    /// The light's shader gel and target 13's binding for its layers.
    gel: Option<(u32, Binding)>,
    ops: Vec<Op>,
}

impl Sequence<'_> {
    /// The sequence's operations before its layers 12 and 13 (`lit`
    /// false: a gel, its casters, freezes and receivers), or those layers'
    /// draws alone (`lit` true), for a held light whose shadow draws ahead
    /// of its run (`tiles`).
    pub(super) fn part(mut self, lit: bool) -> Self {
        let groups = &self.groups;
        self.ops.retain(|op| match op {
            Op::Draw(n, _) => matches!(groups[*n].group.layer, 12 | 13) == lit,
            _ => !lit,
        });
        self
    }
}

impl Glue {
    /// Draws one light's passes of the open view from the scene, as one
    /// sequence ([`ops`]). Returns the layers drawn.
    pub(super) fn scene_light_passes(
        &self,
        mem: &mut dyn GuestMemory,
        passes: &[ScenePass],
        view_target: [u32; 8],
    ) -> Result<usize, Keep> {
        let sequence = self.light_sequence(&*mem, passes)?;
        self.scene_sequence(mem, view_target, sequence, frame_types::Slot::Opaque)
    }

    /// What one light's passes draw as a sequence ([`ops`]).
    pub(super) fn light_sequence<'a>(
        &self,
        mem: &dyn GuestMemory,
        passes: &'a [ScenePass],
    ) -> Result<Sequence<'a>, Keep> {
        let first = passes.first().ok_or(Keep::Unsupported("a scene light with no layers"))?;
        if first.slot != render_bridge::native_lights::LIGHT_SLOT
            && first.slot != render_bridge::native_lights::FIRST_PERSON_SLOT
        {
            return Err(Keep::Unsupported("a scene light outside slots 4 and 10"));
        }
        let (light, gel) = scene_light(&first.light).ok_or(Keep::NoConstants)?;
        let gel = match gel {
            Some((_, shader)) => Some((shader, self.scene_gel(mem, shader)?)),
            None => None,
        };
        let c = constants(&light)?;
        let layers: Vec<_> = passes.iter().map(|p| (p.group.group.layer, p.group.group.pass)).collect();
        Ok(Sequence {
            groups: passes.iter().map(|p| &p.group).collect(),
            light: Some(c),
            filter: None,
            ops: gel_ops(&layers, gel.is_some())?,
            gel,
        })
    }

    /// Draws one object shadow of the open view from the scene, in slot 3
    /// before layer 7 ([`object_ops`]). Returns the layers drawn.
    pub(super) fn scene_object_shadow(
        &self,
        mem: &mut dyn GuestMemory,
        pass: &render_bridge::native_lights::object_shadow_scene::ObjectShadowPass,
        view_target: [u32; 8],
    ) -> Result<usize, Keep> {
        if pass.creature {
            return Err(Keep::Unsupported("a creature's ground shadow draws through its handler"));
        }
        let layers: Vec<_> = pass.groups.iter().map(|g| (g.group.layer, g.group.pass)).collect();
        let sequence = Sequence {
            groups: pass.groups.iter().collect(),
            light: None,
            filter: pass.filter.clone(),
            gel: None,
            ops: object_ops(&layers, pass.filter.is_some())?,
        };
        self.scene_sequence(mem, view_target, sequence, frame_types::Slot::AfterLights)
    }

    fn scene_sequence(
        &self,
        mem: &mut dyn GuestMemory,
        _view_target: [u32; 8],
        sequence: Sequence<'_>,
        slot: frame_types::Slot,
    ) -> Result<usize, Keep> {
        let ready = self.ready(mem)?;
        let (steps, drawn) = self.sequence_steps(&*mem, &ready, sequence)?;
        self.commit_steps(&*mem, &ready, slot, steps)?;
        Ok(drawn)
    }

    /// The steps of one sequence ([`Self::scene_sequence`]), made with
    /// nothing pushed; and the layers they draw.
    pub(super) fn sequence_steps(
        &self,
        mem: &dyn GuestMemory,
        ready: &Arc<Ready>,
        sequence: Sequence<'_>,
    ) -> Result<(Vec<Step>, usize), Keep> {
        let Sequence {
            groups,
            light,
            filter,
            gel,
            ops,
        } = sequence;
        let ready = Arc::clone(ready);
        let view = mem
            .u32(engine::render_loop::occlusion::guest::VIEW_INDEX)
            .unwrap_or(u32::MAX);
        let view_targets = (TargetRef::SceneColour, TargetRef::SceneDepth);
        let caster = groups
            .iter()
            .find(|g| g.group.layer == 10)
            .and_then(|g| g.group.pass);
        // The receivers' map, filled when the freeze records.
        let frozen = caster.map(|_| Frozen::default());
        let mut steps = Vec::with_capacity(ops.len());
        let mut drawn = 0;
        for op in ops {
            let step = match op {
                Op::Clear(n) => Step {
                    targets: game_target(n),
                    stage: Box::new(ClearMap { colour: n == 10 }),
                },
                Op::Freeze(n) => Step {
                    targets: game_target(n),
                    stage: Box::new(FreezeMap {
                        frozen: frozen.clone().ok_or(Keep::Shadow("state: a freeze with no caster pass"))?,
                        kind: if n == 10 { MapKind::ColourMask } else { MapKind::Depth },
                    }),
                },
                Op::Gel => {
                    let (shader, binding) = gel.ok_or(Keep::Unsupported("a gel step with no gel"))?;
                    let ready_gels = ready.lights.get().ok_or(Keep::NoWorld)?;
                    let depth = native_render::gels::quad_depth_of(&ReadMemory(mem))
                        .ok_or(Keep::Unsupported("the gel quad's depth does not read"))?;
                    // The call values the gel's passes may read, and the
                    // render clock its animated passes sample at.
                    let at = native_render::gels::CALL_VALUES;
                    let values = [0, 4, 8, 12].map(|o| mem.u32(at + o).unwrap_or(0));
                    let time = native_render::layer_pass::render_time(&ReadMemory(mem));
                    if let Some(why) = ready_gels.scene_gels.clock_refusal(shader, time) {
                        return Err(Keep::Unsupported(why));
                    }
                    Step {
                        targets: game_target(native_render::gels::TARGET),
                        stage: Box::new(GelPass {
                            gels: Arc::clone(&ready_gels.scene_gels),
                            shader,
                            depth,
                            values,
                            time,
                            guest_size: binding.guest_size,
                        }),
                    }
                }
                Op::Filter => {
                    let filter = filter.clone().ok_or(Keep::Shadow("state: a filter step with no object shadow filter"))?;
                    Step {
                        targets: game_target(10),
                        stage: Box::new(crate::native_shadows_glue::ShadowFilterPass {
                            filter,
                            drawer: crate::native_shadows_glue::FilterDrawer::Scene,
                            chain: None,
                        }),
                    }
                }
                Op::Draw(n, attach) => {
                    let group = groups[n];
                    if group.entries.is_empty() {
                        // The layer's target setup and clear still ran.
                        continue;
                    }
                    let targets = match attach {
                        Attach::View => view_targets,
                        Attach::Target(t) => game_target(t),
                    };
                    // The whole target: the bind's viewport cuts it.
                    let rect = Some(WHOLE_TARGET);
                    // A gel light's layers sample target 13 as its gel step
                    // left it.
                    let binding = gel.map(|(_, b)| b);
                    let prepared = match group.group.layer {
                        12 | 13 => {
                            let c = light.ok_or(Keep::NoConstants)?;
                            let draws = self.scene_light_draws(mem, &ready, group, &c, binding.is_some())?;
                            self.prepare_planned(mem, Arc::clone(&ready), c, draws, binding, rect)?
                        }
                        _ => self.shadow_prepared(
                            mem,
                            view,
                            group,
                            light,
                            ShadowMap::Frozen(frozen.clone()),
                            rect,
                            binding,
                        )?,
                    };
                    let retained = RetainedLight::of(&ready, &prepared)?;
                    drawn += 1;
                    Step {
                        targets,
                        stage: Box::new(RunPass {
                            retained: Some(retained),
                            light: prepared,
                            lighting_only: None,
                        }),
                    }
                }
            };
            steps.push(step);
        }
        Ok((steps, drawn))
    }

    /// Pushes `steps` at `slot` as one sequence, each after a bind of its
    /// attachments, or pushes nothing when the world changed under them.
    pub(super) fn commit_steps(
        &self,
        mem: &dyn GuestMemory,
        ready: &Arc<Ready>,
        slot: frame_types::Slot,
        steps: Vec<Step>,
    ) -> Result<(), Keep> {
        {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !current_ready(&state, mem).is_some_and(|current| Arc::ptr_eq(current, ready)) {
                return Err(Keep::Unsupported("world changed during light preparation"));
            }
        }
        for Step { targets: (colour, depth), stage } in steps {
            frame_builder().push(FrameOp::Bind { colour, depth });
            push(mem, slot, stage);
        }
        Ok(())
    }
}

/// One step of a sequence: the attachments it binds and its stage.
pub(super) struct Step {
    pub(super) targets: (TargetRef, TargetRef),
    pub(super) stage: Box<dyn Stage>,
}

/// A scissor covering any target: the view's rect cuts it
/// ([`RunPass::draws`]).
const WHOLE_TARGET: [u32; 4] = [0, 0, i32::MAX as u32, i32::MAX as u32];

impl Glue {
    /// Target 13's binding for a scene light's gel `shader`, when the map
    /// load translated it ([`crate::native_gels_scene`]) and the target is
    /// enabled: the read-only slot its layers sample, at level 0.
    fn scene_gel(&self, mem: &dyn GuestMemory, shader: u32) -> Result<Binding, Keep> {
        let ready = self.ready(mem)?;
        let world = ready.lights.get().ok_or(Keep::NoWorld)?;
        if let Some(why) = crate::native_gels_scene::refusal(&world.scene_gels, shader) {
            return Err(Keep::Unsupported(why));
        }
        let target = native_render::gels::Target::read(&ReadMemory(mem))
            .ok_or(Keep::Unsupported("unavailable gel target 13"))?;
        let level = target.level(0).ok_or(Keep::Unsupported("unavailable gel target 13"))?;
        let definition = mem.u32(native_render::gels::DEFINITION).unwrap_or(0);
        Ok(Binding {
            key: native_render::gels::Key { sequence: 0, definition, shader, surface: level.surface },
            guest_size: level.size,
        })
    }
}

/// A scene light's shader gel drawn into target 13's level 0
/// ([`Op::Gel`]): its layer-1 passes over the quad helper's strip.
struct GelPass {
    gels: Arc<crate::native_gels_scene::Gels>,
    shader: u32,
    depth: (f32, f32),
    values: [u32; 4],
    time: Option<f64>,
    guest_size: (u32, u32),
}

impl Stage for GelPass {
    fn name(&self) -> &'static str {
        "light gel"
    }

    fn record(&mut self, t: &gfx::external::ExternalAttachments, request: &native_render::program::ViewInputs) -> StageWork {
        if request.guest_size != self.guest_size {
            unimplemented!("lights: target 13 is not the gel's size");
        }
        let target = native_render::screen_gpu::ScreenTarget {
            device: &t.device,
            queue: &t.queue,
            uploads: &t.uploads,
            color: Some(native_render::glow::PostSurface {
                view: &t.color,
                guest_size: self.guest_size,
            }),
            target1: None,
            convolutions: [None; 3],
            z_scale: request.z_scale,
        };
        let commands = match self.gels.prepare(target, self.shader, self.depth, self.values, self.time) {
            Ok(c) => c,
            Err(_) => unimplemented!("lights: the gel's passes do not prepare"),
        };
        StageWork::Encoded(gfx::external::ExternalEncoded {
            ranges: commands.ranges(),
            draws: commands.draws(),
            passes: commands.passes(),
            commands: Box::new(move |encoder| commands.record(encoder)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caster(target: u32, flags: u32) -> Option<ShadowPass> {
        Some(ShadowPass {
            target,
            size: [512, 512],
            flags,
            clip_control: 1,
            raster: 0,
            offset_enable: 1,
            offset_constant: 0,
            offset_slope: 0,
            stencil_mode: 0,
            shadow_function: 0,
        })
    }

    #[test]
    fn a_shadowed_lights_layers_clear_draw_and_freeze_the_map_before_its_receivers() {
        let layers = [(10, caster(9, 4)), (11, caster(2, 4)), (14, caster(1, 4)), (12, None), (13, None)];
        assert_eq!(
            ops(&layers).unwrap(),
            [
                Op::Clear(9),
                Op::Draw(0, Attach::Target(9)),
                Op::Freeze(9),
                Op::Draw(1, Attach::View),
                Op::Draw(2, Attach::Target(1)),
                Op::Draw(3, Attach::View),
                Op::Draw(4, Attach::View),
            ]
        );
    }

    #[test]
    fn a_gel_light_draws_its_gel_into_target_13_before_any_layer() {
        let layers = [(10, caster(9, 4)), (11, caster(2, 4)), (12, None)];
        let ops = gel_ops(&layers, true).unwrap();
        assert_eq!(ops[0], Op::Gel);
        assert_eq!(ops[1..], super::ops(&layers).unwrap()[..]);
        assert_eq!(gel_ops(&layers, false).unwrap(), super::ops(&layers).unwrap());
    }

    #[test]
    fn an_unshadowed_light_draws_its_layers_on_their_targets_with_no_map() {
        let layers = [(11, caster(2, 2)), (14, caster(1, 0)), (12, None)];
        assert_eq!(
            ops(&layers).unwrap(),
            [Op::Draw(0, Attach::View), Op::Draw(1, Attach::Target(1)), Op::Draw(2, Attach::View)]
        );
    }

    #[test]
    fn a_caster_map_freezes_the_casters_region_at_the_targets_scale() {
        // A 1x target the casters fill.
        assert_eq!(frozen_size((0, 0, 512, 512), (512, 512), (512, 512)), Some((512, 512)));
        // At render scale 2 it freezes twice the texels.
        assert_eq!(frozen_size((0, 0, 512, 512), (512, 512), (1024, 1024)), Some((1024, 1024)));
        // A caster map smaller than its target (the shadow setup's size)
        // takes the top-left region, at 1x and scaled.
        assert_eq!(frozen_size((0, 0, 256, 128), (512, 512), (512, 512)), Some((256, 128)));
        assert_eq!(frozen_size((0, 0, 256, 128), (512, 512), (1536, 1536)), Some((768, 384)));
    }

    #[test]
    fn a_caster_region_off_its_targets_corner_or_past_it_freezes_nothing() {
        assert_eq!(frozen_size((8, 0, 256, 256), (512, 512), (512, 512)), None);
        assert_eq!(frozen_size((0, 0, 1024, 512), (512, 512), (512, 512)), None);
        assert_eq!(frozen_size((0, 0, 0, 512), (512, 512), (512, 512)), None);
    }

    #[test]
    fn a_receiver_samples_the_newest_frozen_map() {
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let map = |size| SampledMap {
            kind: MapKind::Depth,
            view: gfx::depth_bits::DepthBits::output(&ctx.device, size).create_view(&Default::default()),
            size,
        };
        let frozen = Frozen::default();
        assert!(frozen.get().is_none(), "nothing frozen yet");
        frozen.put(map((1024, 1024)));
        assert_eq!(frozen.get().unwrap().size, (1024, 1024));
        // A replay freezes again: the map is the newest.
        frozen.put(map((1024, 512)));
        assert_eq!(frozen.get().unwrap().size, (1024, 512));
    }

    #[test]
    fn a_colour_mask_caster_clears_draws_and_freezes_target_10() {
        // Pass flag 4 with 5 clear is the object mask (target 10).
        let mask = caster(10, 1 << 4);
        assert!(mask.unwrap().colour_mask());
        assert_eq!(
            ops(&[(10, mask), (11, mask)]).unwrap(),
            [Op::Clear(10), Op::Draw(0, Attach::Target(10)), Op::Freeze(10), Op::Draw(1, Attach::View)]
        );
    }

    #[test]
    fn a_caster_off_its_maps_target_or_an_unknown_layer_refuses_the_light() {
        assert!(ops(&[(10, caster(10, 4)), (12, None)]).is_err(), "a depth caster on target 10");
        assert!(ops(&[(10, caster(9, 1 << 4)), (12, None)]).is_err(), "a mask on target 9");
        assert!(ops(&[(10, None)]).is_err(), "a caster without its state");
        assert!(ops(&[(12, None), (7, None)]).is_err());
    }

    #[test]
    fn an_object_shadow_filters_its_mask_before_the_freeze_its_receivers_bind() {
        let mask = caster(10, 1 << 4);
        let layers = [(10, mask), (11, caster(2, 1 << 4))];
        assert_eq!(
            object_ops(&layers, true).unwrap(),
            [
                Op::Clear(10),
                Op::Draw(0, Attach::Target(10)),
                Op::Filter,
                Op::Freeze(10),
                Op::Draw(1, Attach::View),
            ]
        );
        assert_eq!(object_ops(&layers, false).unwrap().len(), 4, "no filter, no quads");
        assert!(object_ops(&[(10, caster(9, 4)), (11, None)], true).is_err(), "a depth caster");
        assert!(object_ops(&[(10, mask)], true).is_err(), "no receivers");
        assert!(object_ops(&[(11, None), (10, mask)], true).is_err(), "out of order");
    }
}
