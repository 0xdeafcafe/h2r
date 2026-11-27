//! A cut light's layers drawn from the scene (docs/graphics/native-lights.md,
//! "The light frame under the cut"). The guest's layer begin and end do
//! not run, so the work they make on the device is the light's own steps
//! of one external sequence, each on its explicit attachments
//! (`d3d::Device::prepare_external_sequence`), with nothing rebound:
//!
//! - layer 10: target 9's depth cleared to 1 and stencil 0 (the begin's
//!   `0x00014b60`), the casters drawn into it, and the region they drew
//!   frozen into a map on each target copy ([`Frozen`],
//!   `gfx::depth_bits::DepthBits::convert_region`);
//! - layer 11: the receivers on the view's target, sampling that map;
//! - layer 14: target 1;
//! - layers 12 and 13: the view's target.
//!
//! The sequence is committed whole with no light tail
//! (`d3d::PreparedSequence::commit`), or not at all.

use super::*;
use frame_scene::ShadowPass;
use native_render::shadow_gpu::{MapKind, SampledMap};
use render_bridge::native_lights::ScenePass;

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
    /// Clear a shadow map target's depth to 1 and stencil to 0.
    Clear(u32),
    /// Draw the light's pass `n` (an index into its passes).
    Draw(usize, Attach),
    /// Freeze a shadow map target's depth for the receivers after it.
    Freeze(u32),
}

/// A light's operations from its passes' layers and shadow states, in the
/// passes' order. A colour-mask caster (object shadows only) and a layer
/// the passes do not draw refuse the light.
pub(super) fn ops(layers: &[(u8, Option<ShadowPass>)]) -> Result<Vec<Op>, Keep> {
    let mut out = Vec::with_capacity(layers.len() + 2);
    for (n, &(layer, pass)) in layers.iter().enumerate() {
        match layer {
            10 => {
                let pass = pass.ok_or(Keep::Shadow("state"))?;
                if pass.colour_mask() || pass.target != 9 {
                    return Err(Keep::Unsupported("a scene light's caster is not the depth map"));
                }
                out.extend([Op::Clear(9), Op::Draw(n, Attach::Target(9)), Op::Freeze(9)]);
            }
            11 | 12 | 13 => out.push(Op::Draw(n, Attach::View)),
            14 => out.push(Op::Draw(n, Attach::Target(1))),
            _ => return Err(Keep::Unsupported("a scene light layer the passes do not draw")),
        }
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

/// Clears a shadow map's depth and stencil (the layer begin's clear).
struct ClearDepth;

impl d3d::ExternalPass for ClearDepth {
    fn replays_scene(&self) -> bool {
        true
    }

    fn uses_validated_resources(&self) -> bool {
        true
    }

    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites {
            color: false,
            depth_stencil: true,
        }
    }

    fn record_encoded(
        &mut self,
        t: &d3d::backend::ExternalAttachments,
        _: d3d::ExternalPassRequest,
    ) -> Option<Result<d3d::backend::ExternalEncoded, &'static str>> {
        let depth = t.depth.clone();
        let stencil = t.depth_format.has_stencil_aspect();
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
                drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("scene light shadow clear"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
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

/// Freezes the region of a caster map's depth its casters drew, on each
/// copy at that copy's texels, for the receivers after it ([`Frozen`]).
struct FreezeDepth {
    frozen: Frozen,
}

impl d3d::ExternalPass for FreezeDepth {
    fn replays_scene(&self) -> bool {
        true
    }

    fn uses_validated_resources(&self) -> bool {
        true
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
        let depth = t.depth.texture().clone();
        let Some(size) = frozen_size(&request, t.size)
            .filter(|&(w, h)| w <= depth.width() && h <= depth.height())
        else {
            return Some(Err("a scene light caster map outside its target"));
        };
        if depth.format() != wgpu::TextureFormat::Depth32FloatStencil8
            || !depth.usage().contains(wgpu::TextureUsages::TEXTURE_BINDING)
            || depth.sample_count() != 1
        {
            return Some(Err("a scene light caster map that cannot be sampled"));
        }
        let out = gfx::depth_bits::DepthBits::output(&t.device, size);
        self.frozen.put(
            t.copy,
            SampledMap {
                kind: MapKind::Depth,
                view: out.create_view(&Default::default()),
                size,
            },
        );
        let (bits, device, queue) = (depth_bits(&t.device), t.device.clone(), t.queue.clone());
        Some(Ok(d3d::backend::ExternalEncoded {
            ranges: 1,
            draws: 1,
            passes: 0,
            commands: Box::new(move |encoder| {
                bits.convert_region(&device, &queue, encoder, &depth, &out);
            }),
        }))
    }
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
        let ops = ops(&layers)?;
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
        let caster_size = passes
            .iter()
            .find(|p| p.group.group.layer == 10)
            .and_then(|p| p.group.group.pass)
            .map(|s| (s.size[0], s.size[1]));
        let targets = |n: u32| -> Result<d3d::ExternalTargets, Keep> {
            let [color, depth] = crate::native_shadows_glue::expected_targets(&*mem, n)
                .ok_or(Keep::Shadow("target"))?;
            let (w, h) = match (n, caster_size) {
                (9, Some(size)) => size,
                _ => target_size(&*mem, n).ok_or(Keep::Shadow("target"))?,
            };
            Ok(d3d::ExternalTargets {
                color,
                depth,
                rect: (0, 0, w, h),
            })
        };
        // Each copy's frozen map, filled as that copy records its freeze.
        let frozen = caster_size.map(|_| Frozen::default());
        let plan = |ranges| d3d::ExternalPlan {
            stage: timeline::native::Stage::Lights,
            view,
            ranges,
            slots: Vec::new(),
        };
        let mut steps = Vec::with_capacity(ops.len());
        let mut drawn = 0;
        for op in ops {
            let step = match op {
                Op::Clear(n) => d3d::ExternalStep {
                    targets: targets(n)?,
                    plan: plan(1),
                    pass: Box::new(ClearDepth),
                },
                Op::Freeze(n) => d3d::ExternalStep {
                    targets: targets(n)?,
                    plan: plan(1),
                    pass: Box::new(FreezeDepth {
                        frozen: frozen.clone().ok_or(Keep::Shadow("state"))?,
                    }),
                },
                Op::Draw(n, attach) => {
                    let group = &passes[n].group;
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
                    let prepared = match group.group.layer {
                        12 | 13 => {
                            let draws = self.scene_light_draws(&d, &*mem, &ready, group, &c)?;
                            self.prepare_planned(&d, &*mem, Arc::clone(&ready), c, draws, None, false, rect)?
                        }
                        _ => self.shadow_prepared(
                            &d,
                            &*mem,
                            view,
                            group,
                            Some(c),
                            ShadowMap::Frozen(frozen.clone()),
                            rect,
                        )?,
                    };
                    let retained = RetainedLight::of(&ready, &prepared)?;
                    let ranges = prepared.draws.iter().map(|draw| draw.ranges.len()).sum();
                    drawn += 1;
                    d3d::ExternalStep {
                        targets,
                        plan: plan(ranges),
                        pass: Box::new(RunPass {
                            retained: Some(retained),
                            light: prepared,
                            view: None,
                            clip: None,
                            gel_slot: None,
                            fallback: None,
                            colour_only: false,
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
    fn a_colour_mask_caster_or_an_unknown_layer_refuses_the_light() {
        // Pass flag 4 with 5 clear is the object mask (target 10).
        let mask = caster(10, 1 << 4);
        assert!(mask.unwrap().colour_mask());
        assert!(ops(&[(10, mask), (12, None)]).is_err());
        assert!(ops(&[(10, None)]).is_err(), "a caster without its state");
        assert!(ops(&[(12, None), (7, None)]).is_err());
    }
}
