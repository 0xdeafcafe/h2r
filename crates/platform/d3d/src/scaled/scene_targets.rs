//! A sealed game frame writes the same allocation used by a later guest
//! read. Mixed-grid draws, rect clears, uploads and aliases go through
//! the single-copy backend's usual resolution and transfer paths.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use frame_types::TargetRef;
use gfx::raster::RenderScale;
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

/// Why the scaled frame owner leaves a sealed frame to the guest's own
/// commands, in the order [`Scaled::scene_refusal`] checks
/// (docs/graphics/framescene.md, slice 8, "Fractional grids").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneRefusal {
    /// The grid minifies an axis or magnifies one past [`crate::MAX_SCALE`].
    Grid,
    /// The backend keeps separate reference and display copies.
    TwoCopies,
    /// A scene pass is open, or this frame already failed.
    Busy,
    /// The seal refused the frame (`ReplayFrame::refusals`).
    Sealed,
    /// A 1x legacy E3 view may hold other prior pixels than the reference.
    History,
    /// An attachment differs from the recorded one on either grid.
    Targets,
    /// An earlier live sample no longer names its attachment's image.
    LiveViews,
    /// A sampled texture has no copy for this grid, or never arrived.
    Samples,
    /// A typed stage's colour and depth are on different grids, or the
    /// stage cannot record again.
    Stages,
}

impl SceneRefusal {
    pub const ALL: [Self; 9] = [
        Self::Grid,
        Self::TwoCopies,
        Self::Busy,
        Self::Sealed,
        Self::History,
        Self::Targets,
        Self::LiveViews,
        Self::Samples,
        Self::Stages,
    ];

    /// The counter's suffix (`scene_owned_scaled_refused_<name>`).
    pub fn name(self) -> &'static str {
        match self {
            Self::Grid => "grid",
            Self::TwoCopies => "two_copies",
            Self::Busy => "busy",
            Self::Sealed => "sealed",
            Self::History => "history",
            Self::Targets => "targets",
            Self::LiveViews => "live_views",
            Self::Samples => "samples",
            Self::Stages => "stages",
        }
    }
}

static REFUSED: [AtomicU64; SceneRefusal::ALL.len()] =
    [const { AtomicU64::new(0) }; SceneRefusal::ALL.len()];
static OWNED_WHOLE: AtomicU64 = AtomicU64::new(0);
static OWNED_FRACTIONAL: AtomicU64 = AtomicU64::new(0);

/// The scaled owner's frames since start: owned on a whole-number grid,
/// owned on a fractional one, and refused by reason. Names as in
/// `render.scene` (docs/runtime/agent-api.md, "FrameScene checks").
pub fn scene_counters() -> Vec<(String, u64)> {
    let mut out = vec![
        (
            "scene_owned_scaled_whole".to_owned(),
            OWNED_WHOLE.load(Ordering::Relaxed),
        ),
        (
            "scene_owned_scaled_fractional".to_owned(),
            OWNED_FRACTIONAL.load(Ordering::Relaxed),
        ),
    ];
    out.extend(SceneRefusal::ALL.iter().zip(&REFUSED).map(|(r, n)| {
        (
            format!("scene_owned_scaled_refused_{}", r.name()),
            n.load(Ordering::Relaxed),
        )
    }));
    out
}

fn note_owned(grid: RenderScale) {
    let counter = if grid.uniform_integer().is_some() {
        &OWNED_WHOLE
    } else {
        &OWNED_FRACTIONAL
    };
    counter.fetch_add(1, Ordering::Relaxed);
}

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
        self.scene_refusal(frame, targets).is_none()
    }

    /// [`Self::can_draw_scene`] for the frame owner: a refusal is counted
    /// by its reason ([`scene_counters`]).
    pub fn check_scene(
        &self,
        frame: &ReplayFrame,
        targets: &HashMap<TargetRef, Surface>,
    ) -> bool {
        let Some(refusal) = self.scene_refusal(frame, targets) else {
            return true;
        };
        let i = SceneRefusal::ALL
            .iter()
            .position(|r| *r == refusal)
            .expect("every refusal is listed");
        REFUSED[i].fetch_add(1, Ordering::Relaxed);
        false
    }

    /// The first reason this frame cannot be drawn on these attachments,
    /// or `None`. Any grid from 1x to [`crate::MAX_SCALE`] on each axis
    /// replays, whole or fractional: frame-sized levels are on the display
    /// grid, each smaller level on its own ([`RenderScale::level`]).
    pub fn scene_refusal(
        &self,
        frame: &ReplayFrame,
        targets: &HashMap<TargetRef, Surface>,
    ) -> Option<SceneRefusal> {
        let grid = self.scale;
        let one = grid == RenderScale::ONE;
        if !grid.replayable(crate::MAX_SCALE) {
            return Some(SceneRefusal::Grid);
        }
        if !self.single_copy {
            return Some(SceneRefusal::TwoCopies);
        }
        if self.scene_active || self.frame_failed {
            return Some(SceneRefusal::Busy);
        }
        if !frame.refusals.is_empty() {
            return Some(SceneRefusal::Sealed);
        }
        if one
            && self.lighting == Lighting::E3
            && !targets
                .values()
                .all(|s| self.scene_coherent.contains(&s.reference))
            && !frame.all_targets_defined()
        {
            return Some(SceneRefusal::History);
        }
        let reference = targets
            .iter()
            .map(|(&name, s)| (name, s.reference))
            .collect();
        // The reference store describes guest dimensions. Live samples may
        // reside on the display grid and must be checked in that store.
        if !self
            .reference
            .matches_scene_targets_at(frame, &reference, RenderScale::ONE)
        {
            return Some(SceneRefusal::Targets);
        }
        if !self.scene_live_views_match(frame, targets) {
            return Some(SceneRefusal::LiveViews);
        }
        let targets_match = targets.iter().all(|(name, s)| {
            let desc = match name {
                TargetRef::SceneColour => frame.colour,
                TargetRef::SceneDepth => frame.depth,
                TargetRef::Aux(i) => frame.aux.get(usize::from(*i)).copied(),
                TargetRef::None => None,
            };
            desc.is_some_and(|d| {
                s.guest
                    .is_some_and(|g| self.view.scene_target_matches(g, d, RenderScale::ONE))
                    && self
                        .view_surface(s)
                        .is_none_or(|v| self.view.scene_target_matches(v, d, grid))
            })
        });
        if !targets_match {
            return Some(SceneRefusal::Targets);
        }
        for (_, op) in &frame.ops {
            match op {
                ReplayOp::Draw { textures, .. } => {
                    let sampled = textures.iter().flatten().all(|t| match t {
                        ReplayTexture::Placeholder(_) => false,
                        ReplayTexture::Frozen(sample) => {
                            // One native 1x draw cannot preserve a distinct
                            // legacy E3 image and reference image.
                            (!one
                                || self.lighting != Lighting::E3
                                || sample.images().is_some_and(|images| images.same_copies()))
                                && self.frozen_texture(sample, grid).is_some()
                        }
                        _ => true,
                    });
                    if !sampled {
                        return Some(SceneRefusal::Samples);
                    }
                }
                ReplayOp::Stage(stage) => {
                    // As in external_attachments, all typed attachment views
                    // must inhabit the same raster grid. Sampled slots may
                    // have independent sizes.
                    let raster = |r| {
                        targets.get(&r).map(|s| {
                            self.view_surface(s)
                                .map_or(s.size, |_| grid.size(s.size.0, s.size.1))
                        })
                    };
                    if raster(stage.surfaces.color) != raster(stage.surfaces.depth)
                        || !stage.pass.lock().is_ok_and(|p| p.replays_scene())
                    {
                        return Some(SceneRefusal::Stages);
                    }
                }
                _ => {}
            }
        }
        None
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
        if backend.scale == RenderScale::ONE {
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
        if pass.backend.scale != RenderScale::ONE && pass.backend.lighting == Lighting::E3 {
            // When legacy shadow rendering last owned an off-screen level,
            // a valid scaled depth can be the only copy with those values.
            for s in targets.values() {
                if let Some(i) = pass.backend.shadow_at(&s.reference)
                    && let Some(v) = &pass.backend.shadows[i].view
                    && !v.stale_depth
                    && !v.stale_stencil
                {
                    let (from, to) = (v.depth, pass.backend.guest(*s));
                    <GfxBackend as ScaleView<GfxBackend>>::resize(&mut pass.backend.view, from, to)?;
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

    /// The view's backend, which draws a display frame on its own targets
    /// before this frame presents (`native_render::frame_graph::
    /// draw_scaled_display_at_rate`).
    pub fn view_mut(&mut self) -> &mut GfxBackend {
        &mut self.backend.view
    }

    pub fn apply(&mut self, op: &ReplayOp) -> Result<(), String> {
        let grid = self.backend.scale;
        self.commands
            .apply(self.backend, self.targets, true, grid, op)
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
        if self.backend.scale == RenderScale::ONE {
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
        self,
        flags: u32,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(BatchStats, Outputs<Texture>), String> {
        self.present_inner(flags, gamma, None)
    }

    /// [`Self::present`], with `image` (a display frame the view drew on
    /// its own targets, `GuestDrawPass::into_image`) going to the window in
    /// place of this frame's own view image. The reference and the guest's
    /// attachments are this frame's as ever.
    pub fn present_showing(
        self,
        flags: u32,
        gamma: Option<&[u8; 768]>,
        image: gfx::surface::SurfaceId,
    ) -> Result<(BatchStats, Outputs<Texture>), String> {
        self.present_inner(flags, gamma, Some(image))
    }

    fn present_inner(
        mut self,
        flags: u32,
        gamma: Option<&[u8; 768]>,
        showing: Option<gfx::surface::SurfaceId>,
    ) -> Result<(BatchStats, Outputs<Texture>), String> {
        let shown = *self
            .targets
            .get(&self.frame.presented)
            .ok_or("missing scene present target")?;
        let image = showing.unwrap_or_else(|| self.backend.image(shown));
        if self.backend.lighting == Lighting::E3 {
            // Feed the original E3 view when it retakes its attachments.
            // At 1x it holds separate frame-sized copies. An off-screen
            // shadow instead refills lazily on its next legacy use.
            for key in writable(self.frame) {
                let s = self.targets[&key];
                if self.backend.scale == RenderScale::ONE
                    && let Some(v) = self.backend.view_surface(&s)
                {
                    let guest = self.backend.guest(s);
                    <GfxBackend as ScaleView<GfxBackend>>::resize(&mut self.backend.view, guest, v)?;
                }
                self.backend.shadow_stale(&s.reference);
            }
            // An escaped live colour view must continue following the
            // original view allocation after the next legacy draw.
            if self.backend.scale == RenderScale::ONE {
                for (_, op) in &self.frame.ops {
                    if let ReplayOp::Sample {
                        of,
                        id,
                        how: ReplaySample::InPlace,
                        ..
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
        if self.backend.scale == RenderScale::ONE {
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
        if let Some(image) = showing {
            self.backend.view.retain_shown(image);
        }
        if self.backend.scale == RenderScale::ONE && self.backend.lighting == Lighting::E3 {
            self.backend
                .scene_coherent
                .extend(self.targets.values().map(|s| s.reference));
        }
        self.backend.stats.view_frames += 1;
        note_owned(self.backend.scale);
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
        private_lighting: Option<crate::Lighting>,
    ) -> Result<usize, String> {
        let surfaces = surfaces
            .try_map(|s| Some(self.image(*s)))
            .expect("all scene targets are live");
        let relit = self.view.record_replay_stage(stage, surfaces, private_lighting)?;
        self.stats.display_draws += stage.plan.ranges as u64;
        Ok(relit)
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
        _grid: RenderScale,
    ) -> Option<Texture> {
        let images = sample.images()?;
        let reference = images.reference.for_device(&self.reference.ctx.device)?;
        let view = if self.scale == RenderScale::ONE {
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
        _grid: RenderScale,
    ) -> Result<(), String> {
        if self.write_surface(target, desc, bytes)? {
            Ok(())
        } else {
            Err("scene upload refused".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_refusal_has_its_own_counter() {
        let names: HashSet<_> = SceneRefusal::ALL.iter().map(|r| r.name()).collect();
        assert_eq!(names.len(), SceneRefusal::ALL.len());
        let counters = scene_counters();
        assert_eq!(counters.len(), 2 + SceneRefusal::ALL.len());
        for r in SceneRefusal::ALL {
            let name = format!("scene_owned_scaled_refused_{}", r.name());
            assert!(counters.iter().any(|(n, _)| *n == name), "{name}");
        }
    }

    #[test]
    fn owned_frames_count_by_grid_kind() {
        let count = |name: &str| {
            scene_counters()
                .into_iter()
                .find(|(n, _)| n == name)
                .unwrap()
                .1
        };
        let whole = count("scene_owned_scaled_whole");
        let fractional = count("scene_owned_scaled_fractional");
        note_owned(RenderScale::from(2));
        note_owned(RenderScale::for_output(1366, 768).unwrap());
        assert!(count("scene_owned_scaled_whole") > whole);
        assert!(count("scene_owned_scaled_fractional") > fractional);
    }
}
