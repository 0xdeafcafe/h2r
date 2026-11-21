//! A sealed game frame writes the same allocation used by a later guest
//! read. Mixed-grid draws, rect clears, uploads and aliases go through
//! the single-copy backend's usual resolution and transfer paths.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use frame_scene::TargetRef;
use gfx::surface::SurfaceId;
use gfx::{TextureDesc, texture::DecodedTexture};

use super::{Lighting, ScaleView, ScaledBackend, ScaledSurface, ScaledTexture};
use crate::backend::{Backend, ExternalSurfaces, SceneStage};
use crate::gfx_backend::guest_draw_pass::commands::{Commands, SceneDevice};
use crate::gfx_backend::{BatchStats, GfxBackend, HostTexture};
use crate::scene_sink::{ReplayFrame, ReplayOp, ReplaySample, ReplayTexture};
use crate::threaded::owner::Outputs;

type Scaled = ScaledBackend<GfxBackend, GfxBackend>;
type Surface = ScaledSurface<SurfaceId, SurfaceId>;
type Texture = ScaledTexture<HostTexture, HostTexture>;

impl Scaled {
    /// The worker checks before releasing the original commands. The
    /// reference describes all logical attachments, including unscaled
    /// auxiliaries. A 1x legacy E3 view can have different previous
    /// pixels, so require this interval to define every output itself.
    pub fn can_draw_scene(
        &self,
        frame: &ReplayFrame,
        targets: &HashMap<TargetRef, Surface>,
    ) -> bool {
        if !self.single_copy
            || self.scene_active
            || self.frame_failed
            || !frame.refusals.is_empty()
            || (self.scale == 1
                && self.lighting == Lighting::E3
                && !targets
                    .values()
                    .all(|s| self.scene_coherent.contains(&s.reference))
                && !frame.all_targets_defined())
        {
            return false;
        }
        let reference = targets
            .iter()
            .map(|(&name, s)| (name, s.reference))
            .collect();
        self.reference.matches_scene_targets(frame, &reference)
            && targets.iter().all(|(name, s)| {
                let desc = match name {
                    TargetRef::SceneColour => frame.colour,
                    TargetRef::SceneDepth => frame.depth,
                    TargetRef::Aux(i) => frame.aux.get(usize::from(*i)).copied(),
                    TargetRef::None => None,
                };
                desc.is_some_and(|d| {
                    s.guest
                        .is_some_and(|g| self.view.scene_target_matches(g, d, 1))
                        && self
                            .view_surface(s)
                            .is_none_or(|v| self.view.scene_target_matches(v, d, self.scale))
                })
            })
            && frame.ops.iter().all(|(_, op)| match op {
                ReplayOp::Draw { textures, .. } => textures.iter().flatten().all(|t| match t {
                    ReplayTexture::Placeholder(_) => false,
                    ReplayTexture::Frozen(sample) => {
                        // One native 1x draw cannot preserve a distinct
                        // legacy E3 image and reference image.
                        (self.scale != 1
                            || self.lighting != Lighting::E3
                            || sample.images().is_some_and(|images| images.same_copies()))
                            && self.frozen_texture(sample, self.scale).is_some()
                    }
                    _ => true,
                }),
                ReplayOp::Stage(stage) => {
                    // As in external_attachments, all typed attachment views
                    // must inhabit the same raster grid. Sampled slots may
                    // have independent sizes.
                    let raster = |r| {
                        targets.get(&r).map(|s| {
                            if self.scale > 1 {
                                self.view_surface(s).map_or((s.size.0, s.size.1), |_| {
                                    (s.size.0 * self.scale, s.size.1 * self.scale)
                                })
                            } else {
                                s.size
                            }
                        })
                    };
                    raster(stage.surfaces.color) == raster(stage.surfaces.depth)
                        && stage.pass.lock().is_ok_and(|p| p.replays_scene())
                }
                _ => true,
            })
    }
}

fn writable(frame: &ReplayFrame) -> HashSet<TargetRef> {
    let mut written = HashSet::new();
    let mut bound = None;
    for (_, op) in &frame.ops {
        match op {
            ReplayOp::Bind { colour, depth } => bound = Some((*colour, *depth)),
            ReplayOp::Draw { draw, .. } => {
                if let Some((c, z)) = bound {
                    if draw.state.render.color_write.xbox_value() != 0 {
                        written.insert(c);
                    }
                    if draw.writes_depth_stencil() {
                        written.insert(z);
                    }
                }
            }
            ReplayOp::Clear {
                colour,
                depth,
                clear,
            } => {
                if clear.color.is_some() {
                    written.insert(*colour);
                }
                if clear.depth.is_some() || clear.stencil.is_some() {
                    written.insert(*depth);
                }
            }
            ReplayOp::ClearRects {
                colour,
                depth,
                clear,
            } => {
                if clear.color.is_some() {
                    written.insert(*colour);
                }
                if clear.depth.is_some() || clear.stencil.is_some() {
                    written.insert(*depth);
                }
            }
            ReplayOp::Stage(stage) => {
                written.extend([stage.surfaces.color, stage.surfaces.depth]);
                for (slot, target) in stage.plan.slots.iter().zip(&stage.surfaces.slots) {
                    if slot.is_some_and(|p| p.written) {
                        written.extend(*target);
                    }
                }
            }
            ReplayOp::Copy { to, .. } | ReplayOp::Upload { to, .. } => {
                written.insert(*to);
            }
            _ => {}
        }
    }
    written.remove(&TargetRef::None);
    written
}

/// A single recording even when the colour and depth render at different
/// resolutions. Drop restores the original owner of the visibility
/// accumulator and shader mode if any command refuses.
pub struct ScaledScenePass<'a> {
    backend: &'a mut Scaled,
    frame: &'a ReplayFrame,
    targets: &'a HashMap<TargetRef, Surface>,
    commands: Commands<Scaled>,
    active: bool,
}

impl<'a> ScaledScenePass<'a> {
    pub fn new(
        backend: &'a mut Scaled,
        frame: &'a ReplayFrame,
        targets: &'a HashMap<TargetRef, Surface>,
    ) -> Result<Self, String> {
        if !backend.can_draw_scene(frame, targets) {
            return Err("scaled scene attachments cannot be adopted".into());
        }
        backend.reference.flush()?;
        backend.scene_coherent.clear();
        if backend.scale == 1 {
            backend.view.move_counter(&mut backend.reference, true);
        }
        backend.view.prune_scene_uploads();
        backend.view.set_scene_receiver(false);
        backend.scene_active = true;
        let pass = Self {
            backend,
            frame,
            targets,
            commands: Commands::default(),
            active: true,
        };
        if pass.backend.scale > 1 && pass.backend.lighting == Lighting::E3 {
            // When legacy shadow rendering last owned an off-screen level,
            // a valid scaled depth can be the only copy with those values.
            for s in targets.values() {
                if let Some(i) = pass.backend.shadow_at(&s.reference)
                    && let Some(v) = &pass.backend.shadows[i].view
                    && !v.stale_depth
                    && !v.stale_stencil
                {
                    let (from, to) = (v.depth, pass.backend.guest(*s));
                    pass.backend.view.resize(from, to)?;
                }
            }
        }
        Ok(pass)
    }

    /// The legacy shadow handoff has resolved any guest-size inputs. The
    /// first graph op has not yet run, so these are exactly its inputs.
    pub fn freeze_inputs(
        &mut self,
        names: &[TargetRef],
    ) -> Result<HashMap<TargetRef, crate::gfx_backend::FrozenTarget>, String> {
        self.backend
            .freeze_view_inputs(self.frame, self.targets, names)
    }

    pub fn apply(&mut self, op: &ReplayOp) -> Result<(), String> {
        let scale = self.backend.scale;
        self.commands
            .apply(self.backend, self.targets, true, scale, op)
    }

    pub fn read_target(&mut self, target: TargetRef) -> Result<([u32; 2], Vec<u8>), String> {
        let s = *self
            .targets
            .get(&target)
            .ok_or("missing scene snapshot target")?;
        let id = self.backend.image(s);
        self.backend.view.read_scene_target(id)
    }

    fn restore(&mut self) {
        if !self.active {
            return;
        }
        self.backend.scene_active = false;
        if self.backend.scale == 1 {
            self.backend
                .view
                .move_counter(&mut self.backend.reference, false);
        }
        self.backend
            .view
            .set_scene_receiver(self.backend.lighting == Lighting::E3);
        self.active = false;
    }

    pub fn present(
        mut self,
        flags: u32,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(BatchStats, Outputs<Texture>), String> {
        let shown = *self
            .targets
            .get(&self.frame.presented)
            .ok_or("missing scene present target")?;
        let image = self.backend.image(shown);
        if self.backend.lighting == Lighting::E3 {
            // Feed the original E3 view when it retakes its attachments.
            // At 1x it holds separate frame-sized copies. An off-screen
            // shadow instead refills lazily on its next legacy use.
            for key in writable(self.frame) {
                let s = self.targets[&key];
                if self.backend.scale == 1
                    && let Some(v) = self.backend.view_surface(&s)
                {
                    let guest = self.backend.guest(s);
                    self.backend.view.resize(guest, v)?;
                }
                self.backend.shadow_stale(&s.reference);
            }
            // An escaped live colour view must continue following the
            // original view allocation after the next legacy draw.
            if self.backend.scale == 1 {
                for (_, op) in &self.frame.ops {
                    if let ReplayOp::Sample {
                        of,
                        id,
                        how: ReplaySample::InPlace,
                    } = op
                        && let Some(v) = self.backend.view_surface(&self.targets[of])
                    {
                        let t = self
                            .backend
                            .view
                            .surface_texture(v)?
                            .ok_or("missing scaled live sample")?;
                        self.commands
                            .outputs
                            .samples
                            .get_mut(id)
                            .ok_or("missing scene sample")?
                            .view = Some(t);
                    }
                }
            }
        }
        if self.backend.scale == 1 {
            // The reference still advances its shader/frame/capture
            // boundary, as in a sleeping view's Swap. Both see the same
            // GPU allocation, already recorded once by the display.
            self.backend.view.flush()?;
            self.backend.reference.submit_present(
                None,
                shown.reference,
                flags,
                gamma,
                false,
                self.backend.lighting,
            )?;
        }
        self.backend.view.submit_present(
            None,
            image,
            flags,
            gamma,
            false,
            self.backend.lighting,
        )?;
        if self.backend.scale == 1 && self.backend.lighting == Lighting::E3 {
            self.backend
                .scene_coherent
                .extend(self.targets.values().map(|s| s.reference));
        }
        self.backend.stats.view_frames += 1;
        let stats = self.backend.view.batch_stats();
        self.restore();
        self.backend.finish_frame();
        Ok((stats, std::mem::take(&mut self.commands.outputs)))
    }
}

impl Drop for ScaledScenePass<'_> {
    fn drop(&mut self) {
        // Close draws and counted commands while their counter still
        // belongs to this encoder, including after a mid-frame failure.
        if self.active {
            let _ = self.backend.view.flush();
        }
        for (_, ticket) in self.commands.outputs.tickets.drain(..) {
            self.backend.drop_pixel_count(ticket);
        }
        self.restore();
    }
}

impl SceneDevice for Scaled {
    fn record_scene_stage(
        &mut self,
        stage: &SceneStage<TargetRef>,
        surfaces: ExternalSurfaces<Surface>,
    ) -> Result<(), String> {
        let surfaces = surfaces
            .try_map(|s| Some(self.image(*s)))
            .expect("all scene targets are live");
        self.view.record_replay_stage(stage, surfaces)?;
        self.stats.display_draws += stage.plan.ranges as u64;
        Ok(())
    }
    fn scene_upload(&mut self, asset: &Arc<DecodedTexture>) -> Result<Texture, String> {
        let t = self.view.scene_upload(asset)?;
        Ok(ScaledTexture {
            reference: t.shared(),
            view: Some(t),
        })
    }
    fn frozen_texture(
        &self,
        sample: &crate::scene_sink::FrozenSample,
        _scale: u32,
    ) -> Option<Texture> {
        let images = sample.images()?;
        let reference = images.reference.for_device(&self.reference.ctx.device)?;
        let view = if self.scale == 1 {
            &images.reference
        } else {
            images.display.as_ref()?
        };
        Some(ScaledTexture {
            reference,
            view: Some(view.for_device(&self.view.ctx.device)?),
        })
    }
    fn share_scene_texture(t: &Texture) -> Texture {
        ScaledTexture {
            reference: t.reference.shared(),
            view: t.view.as_ref().map(HostTexture::shared),
        }
    }
    fn write_scene_target(
        &mut self,
        target: Surface,
        desc: &TextureDesc,
        bytes: &[u8],
        _scale: u32,
    ) -> Result<(), String> {
        if self.write_surface(target, desc, bytes)? {
            Ok(())
        } else {
            Err("scene upload refused".into())
        }
    }
}
