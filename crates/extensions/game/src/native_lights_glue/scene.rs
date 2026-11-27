//! A cut light's layers drawn from the scene (docs/graphics/native-lights.md,
//! "The light frame under the cut"). The guest's layer begin and end do
//! not run, so the work they make on the device is the light's own steps
//! of one external sequence, each on its explicit attachments
//! (`d3d::Device::prepare_external_sequence`), with nothing rebound:
//!
//! - layer 10: target 9's depth cleared to 1 and stencil 0 (the begin's
//!   `0x00014b60`), the casters drawn into it, and the region they drew
//!   frozen into a map on each target copy ([`Frozen`],
//!   `gfx::depth_bits::DepthBits::convert_region`); a colour-mask caster
//!   the same on target 10, cleared white with zero alpha, its colour
//!   copied (an object shadow's filter runs between the casters and the
//!   copy, [`object_ops`]);
//! - layer 11: the receivers on the view's target, sampling that map;
//! - layer 14: target 1;
//! - layers 12 and 13: the view's target.
//!
//! The sequence is committed whole with no light tail
//! (`d3d::PreparedSequence::commit`), or not at all.

use super::*;
use frame_scene::ShadowPass;
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

/// One ordered device operation of a light drawn from the scene.
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
}

/// The map target of a caster's pass: 9 for the depth map, 10 for the
/// colour mask (pass flag 4 set with 5 clear).
fn caster_target(pass: Option<ShadowPass>) -> Result<u32, Keep> {
    let pass = pass.ok_or(Keep::Shadow("state"))?;
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
            11 | 12 | 13 => out.push(Op::Draw(n, Attach::View)),
            14 => out.push(Op::Draw(n, Attach::Target(1))),
            _ => return Err(Keep::Unsupported("a scene light layer the passes do not draw")),
        }
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
        let at = out.iter().position(|op| *op == Op::Freeze(10)).ok_or(Keep::Shadow("state"))?;
        out.insert(at, Op::Filter);
    }
    Ok(out)
}

/// The guest size of render target `n`: its `+0x80` and `+0x84` words.
fn target_size(mem: &dyn GuestMemory, n: u32) -> Option<(u32, u32)> {
    use engine::shadow_constants::global::{TARGETS, TARGET_STRIDE};
    let entry = TARGETS.checked_add(n.checked_mul(TARGET_STRIDE)?)?;
    let size = (
        mem.u32(entry.checked_add(0x80)?).ok()?,
        mem.u32(entry.checked_add(0x84)?).ok()?,
    );
    (size.0 != 0 && size.1 != 0 && size.0 <= i32::MAX as u32 && size.1 <= i32::MAX as u32)
        .then_some(size)
}

/// Clears a shadow map (the layer begin's clear): target 9's depth and
/// stencil, or target 10's colour (`colour`).
struct ClearMap {
    colour: bool,
}

impl d3d::ExternalPass for ClearMap {
    fn replays_scene(&self) -> bool {
        true
    }

    fn uses_validated_resources(&self) -> bool {
        true
    }

    fn colour_only(&self) -> bool {
        self.colour
    }

    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites {
            color: self.colour,
            depth_stencil: !self.colour,
        }
    }

    fn record_encoded(
        &mut self,
        t: &d3d::backend::ExternalAttachments,
        _: d3d::ExternalPassRequest,
    ) -> Option<Result<d3d::backend::ExternalEncoded, &'static str>> {
        let (colour, depth) = (self.colour, t.depth.clone());
        let stencil = t.depth_format.has_stencil_aspect();
        let color = t.color.clone();
        Some(Ok(d3d::backend::ExternalEncoded {
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
        }))
    }
}

/// The colour mask's clear: white, zero alpha (native-shadows.md).
const MASK_CLEAR: wgpu::Color = wgpu::Color {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 0.0,
};

/// The caster maps a light froze, one per target copy: each copy records
/// the whole sequence on its own attachments, its freeze before its
/// receivers, so a receiver samples the map of the copy it records on.
#[derive(Clone, Default)]
pub(super) struct Frozen(Arc<Mutex<Vec<(d3d::TargetCopy, SampledMap)>>>);

impl Frozen {
    fn put(&self, copy: d3d::TargetCopy, map: SampledMap) {
        let mut maps = self.0.lock().unwrap_or_else(|e| e.into_inner());
        maps.retain(|(c, _)| *c != copy);
        maps.push((copy, map));
    }

    /// The map last frozen on `copy`.
    pub(super) fn on(&self, copy: d3d::TargetCopy) -> Option<SampledMap> {
        let maps = self.0.lock().unwrap_or_else(|e| e.into_inner());
        maps.iter().find(|(c, _)| *c == copy).map(|(_, map)| map.clone())
    }
}

/// The texels of a copy's caster map: the caster's guest rect (from the
/// target's top-left) scaled from the target's guest size to the copy's
/// `texels`. `None` for an empty or offset rect, or one past the target.
pub(super) fn frozen_size(request: &d3d::ExternalPassRequest, texels: (u32, u32)) -> Option<(u32, u32)> {
    let (x, y, w, h) = request.rect;
    let (gw, gh) = request.surface_size;
    if x != 0 || y != 0 || w == 0 || h == 0 || w > gw || h > gh {
        return None;
    }
    let [_, _, sw, sh] = d3d::external_viewport(request, texels);
    let size = (sw as u32, sh as u32);
    (size.0 != 0 && size.1 != 0 && size.0 <= texels.0 && size.1 <= texels.1).then_some(size)
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

/// Freezes the region of a caster map its casters drew, on each copy at
/// that copy's texels, for the receivers after it ([`Frozen`]): target 9's
/// depth as its bits, target 10's colour mask as a copy.
struct FreezeMap {
    frozen: Frozen,
    kind: MapKind,
}

impl d3d::ExternalPass for FreezeMap {
    fn replays_scene(&self) -> bool {
        true
    }

    fn uses_validated_resources(&self) -> bool {
        true
    }

    fn colour_only(&self) -> bool {
        self.kind == MapKind::ColourMask
    }

    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites {
            color: false,
            depth_stencil: false,
        }
    }

    fn record_encoded(
        &mut self,
        t: &d3d::backend::ExternalAttachments,
        request: d3d::ExternalPassRequest,
    ) -> Option<Result<d3d::backend::ExternalEncoded, &'static str>> {
        let source = match self.kind {
            MapKind::Depth => t.depth.texture().clone(),
            MapKind::ColourMask => t.color.texture().clone(),
        };
        let Some(size) = frozen_size(&request, t.size)
            .filter(|&(w, h)| w <= source.width() && h <= source.height())
        else {
            return Some(Err("a scene light caster map outside its target"));
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
            return Some(Err("a scene light caster map that cannot be sampled"));
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
            t.copy,
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
        Some(Ok(d3d::backend::ExternalEncoded {
            ranges: 1,
            draws: 1,
            passes: 0,
            commands,
        }))
    }
}

/// What a sequence draws from the scene: the groups its operations index,
/// the light's constants (none for an object shadow) and an object
/// shadow's filter.
struct Sequence<'a> {
    groups: Vec<&'a SceneGroup>,
    light: Option<LightConstants>,
    filter: Option<frame_scene::ShadowFilter>,
    ops: Vec<Op>,
}

impl<B: d3d::backend::Backend + Send + 'static> Glue<B> {
    /// Draws one light's passes of the open view from the scene, as one
    /// sequence ([`ops`]). Returns the layers drawn.
    pub(super) fn scene_light_passes(
        &self,
        mem: &mut dyn GuestMemory,
        passes: &[ScenePass],
        view_target: [u32; 8],
    ) -> Result<usize, Keep> {
        let first = passes.first().ok_or(Keep::Unsupported("a scene light with no layers"))?;
        if first.slot != render_bridge::native_lights::LIGHT_SLOT {
            return Err(Keep::Unsupported("a first-person scene light (slot 10)"));
        }
        let (light, gel) = scene_light(&first.light).ok_or(Keep::NoConstants)?;
        if gel.is_some() {
            return Err(Keep::Unsupported("a shader gel draws through its handler"));
        }
        let c = constants(&light)?;
        let layers: Vec<_> = passes.iter().map(|p| (p.group.group.layer, p.group.group.pass)).collect();
        let sequence = Sequence {
            groups: passes.iter().map(|p| &p.group).collect(),
            light: Some(c),
            filter: None,
            ops: ops(&layers)?,
        };
        self.scene_sequence(mem, view_target, sequence, timeline::native::Stage::Lights)
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
            ops: object_ops(&layers, pass.filter.is_some())?,
        };
        self.scene_sequence(mem, view_target, sequence, timeline::native::Stage::Shadows)
    }

    fn scene_sequence(
        &self,
        mem: &mut dyn GuestMemory,
        view_target: [u32; 8],
        sequence: Sequence<'_>,
        stage: timeline::native::Stage,
    ) -> Result<usize, Keep> {
        let Sequence {
            groups,
            light,
            filter,
            ops,
        } = sequence;
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
        let ready = self.ready(mem)?;
        let view = mem
            .u32(engine::render_loop::occlusion::guest::VIEW_INDEX)
            .unwrap_or(u32::MAX);
        let [color, depth, x, y, w, h, _, _] = view_target;
        let view_targets = d3d::ExternalTargets {
            color,
            depth,
            rect: (x, y, w, h),
        };
        // The caster's map size is its pass's: the shadow setup's resize,
        // which the cut frame did not write.
        let caster = groups
            .iter()
            .find(|g| g.group.layer == 10)
            .and_then(|g| g.group.pass);
        let caster_size = caster.map(|s| (s.size[0], s.size[1]));
        let targets = |n: u32| -> Result<d3d::ExternalTargets, Keep> {
            let [color, depth] = crate::native_shadows_glue::expected_targets(&*mem, n)
                .ok_or(Keep::Shadow("target"))?;
            let (w, h) = match (n, caster_size) {
                (9 | 10, Some(size)) => size,
                _ => target_size(&*mem, n).ok_or(Keep::Shadow("target"))?,
            };
            Ok(d3d::ExternalTargets {
                color,
                depth,
                rect: (0, 0, w, h),
            })
        };
        // Each copy's frozen map, filled as that copy records its freeze.
        let frozen = caster.map(|_| Frozen::default());
        let plan = |ranges, slots| d3d::ExternalPlan {
            stage,
            view,
            ranges,
            slots,
        };
        let mut steps = Vec::with_capacity(ops.len());
        let mut drawn = 0;
        for op in ops {
            let step = match op {
                Op::Clear(n) => d3d::ExternalStep {
                    targets: targets(n)?,
                    plan: plan(1, Vec::new()),
                    pass: Box::new(ClearMap { colour: n == 10 }),
                },
                Op::Freeze(n) => d3d::ExternalStep {
                    targets: targets(n)?,
                    plan: plan(1, Vec::new()),
                    pass: Box::new(FreezeMap {
                        frozen: frozen.clone().ok_or(Keep::Shadow("state"))?,
                        kind: if n == 10 { MapKind::ColourMask } else { MapKind::Depth },
                    }),
                },
                Op::Filter => {
                    let filter = filter.clone().ok_or(Keep::Shadow("state"))?;
                    let slot = |n| {
                        crate::native_shadows_glue::target_header(&*mem, n)
                            .map(|header| Some(d3d::ExternalSlot { header, written: true }))
                            .ok_or(Keep::Shadow("target"))
                    };
                    d3d::ExternalStep {
                        targets: targets(10)?,
                        plan: plan(filter.draws.len().max(1), vec![slot(10)?, slot(11)?]),
                        pass: Box::new(crate::native_shadows_glue::ShadowFilterPass {
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
                        Attach::Target(t) => targets(t)?,
                    };
                    let (tx, ty, tw, th) = targets.rect;
                    let rect = Some([tx, ty, tw, th]);
                    let colour_only = group.group.layer == 10 && group.group.pass.is_some_and(|p| p.colour_mask());
                    let prepared = match group.group.layer {
                        12 | 13 => {
                            let c = light.ok_or(Keep::NoConstants)?;
                            let draws = self.scene_light_draws(&d, &*mem, &ready, group, &c)?;
                            self.prepare_planned(&d, &*mem, Arc::clone(&ready), c, draws, None, false, rect)?
                        }
                        _ => self.shadow_prepared(
                            &d,
                            &*mem,
                            view,
                            group,
                            light,
                            ShadowMap::Frozen(frozen.clone()),
                            rect,
                        )?,
                    };
                    let retained = RetainedLight::of(&ready, &prepared)?;
                    let ranges = prepared.draws.iter().map(|draw| draw.ranges.len()).sum();
                    drawn += 1;
                    d3d::ExternalStep {
                        targets,
                        plan: plan(ranges, Vec::new()),
                        pass: Box::new(RunPass {
                            retained: Some(retained),
                            light: prepared,
                            view: None,
                            clip: None,
                            gel_slot: None,
                            fallback: None,
                            colour_only,
                            lighting_only: None,
                            cone_shadow: None,
                        }),
                    }
                }
            };
            steps.push(step);
        }
        {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !current_ready(&state, mem).is_some_and(|current| Arc::ptr_eq(current, &ready)) {
                return Err(Keep::Unsupported("world changed during light preparation"));
            }
        }
        d.prepare_external_sequence(&mut DeviceMemory(mem), steps)
            .map_err(|_| Keep::Unsupported("device error preparing the scene light"))?
            .ok_or(Keep::Unsupported("the scene light is not ready on a target copy"))?
            .commit();
        Ok(drawn)
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
    fn an_unshadowed_light_draws_its_layers_on_their_targets_with_no_map() {
        let layers = [(11, caster(2, 2)), (14, caster(1, 0)), (12, None)];
        assert_eq!(
            ops(&layers).unwrap(),
            [Op::Draw(0, Attach::View), Op::Draw(1, Attach::Target(1)), Op::Draw(2, Attach::View)]
        );
    }

    fn request(rect: (u32, u32, u32, u32), surface: (u32, u32)) -> d3d::ExternalPassRequest {
        d3d::ExternalPassRequest {
            rect,
            surface_size: surface,
            z_scale: 1.0,
            display_projection_factor: 1.0,
        }
    }

    #[test]
    fn each_copy_freezes_the_casters_region_at_its_own_scale() {
        // A 1x copy of a target the casters fill.
        assert_eq!(frozen_size(&request((0, 0, 512, 512), (512, 512)), (512, 512)), Some((512, 512)));
        // The display copy at render scale 2 freezes twice the texels.
        assert_eq!(frozen_size(&request((0, 0, 512, 512), (512, 512)), (1024, 1024)), Some((1024, 1024)));
        // A caster map smaller than its target (the shadow setup's size)
        // takes the top-left region, at 1x and scaled.
        assert_eq!(frozen_size(&request((0, 0, 256, 128), (512, 512)), (512, 512)), Some((256, 128)));
        assert_eq!(frozen_size(&request((0, 0, 256, 128), (512, 512)), (1536, 1536)), Some((768, 384)));
    }

    #[test]
    fn a_caster_region_off_its_targets_corner_or_past_it_freezes_nothing() {
        assert_eq!(frozen_size(&request((8, 0, 256, 256), (512, 512)), (512, 512)), None);
        assert_eq!(frozen_size(&request((0, 0, 1024, 512), (512, 512)), (512, 512)), None);
        assert_eq!(frozen_size(&request((0, 0, 0, 512), (512, 512)), (512, 512)), None);
    }

    #[test]
    fn a_receiver_samples_the_map_frozen_on_its_own_copy() {
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let map = |size| SampledMap {
            kind: MapKind::Depth,
            view: gfx::depth_bits::DepthBits::output(&ctx.device, size).create_view(&Default::default()),
            size,
        };
        let frozen = Frozen::default();
        assert!(frozen.on(d3d::TargetCopy::Display).is_none(), "nothing frozen yet");
        frozen.put(d3d::TargetCopy::Reference, map((512, 512)));
        frozen.put(d3d::TargetCopy::Display, map((1024, 1024)));
        assert_eq!(frozen.on(d3d::TargetCopy::Reference).unwrap().size, (512, 512));
        assert_eq!(frozen.on(d3d::TargetCopy::Display).unwrap().size, (1024, 1024));
        // A replay freezes again: the copy's map is the newest.
        frozen.put(d3d::TargetCopy::Display, map((1024, 512)));
        assert_eq!(frozen.on(d3d::TargetCopy::Display).unwrap().size, (1024, 512));
        assert_eq!(frozen.0.lock().unwrap().len(), 2);
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
