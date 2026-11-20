//! The frame owner's guest draws. Each playback has its own attachments
//! and sampled-target versions. Draw preparation, upload rings and the
//! encoder are the same ones the renderer uses for typed scene stages.

pub(crate) mod commands;
mod typed;
use commands::{Commands, SceneDevice};

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use frame_scene::{AuxTarget, TargetRef};
use gfx::surface::SurfaceId;
use gfx::texture::DecodedTexture;
use gfx::{D3DFormat, TextureDesc};

use super::{GfxBackend, HostTexture};
use crate::backend::{Backend, DrawOutcome};
use crate::scaled::ScaleView;
use crate::scene_sink::{ReplayFrame, ReplayOp, ReplaySample, ReplayTexture};

/// A frame keeps uploads alive by Arc. Sharing the allocation identifies
/// the exact decoded texels without hashing guest assets at every draw.
pub(super) type TextureCache = HashMap<usize, (Weak<DecodedTexture>, HostTexture)>;

/// Records one sealed frame at display scale. Private captures and window
/// frames have disjoint render targets even when they use the same device.
pub struct GuestDrawPass<'a> {
    gpu: &'a mut GfxBackend,
    scale: u32,
    targets: HashMap<TargetRef, SurfaceId>,
    commands: Commands<GfxBackend>,
    presented: TargetRef,
    display: bool,
    lighting: crate::Lighting,
    old_count_samples: bool,
    old_gpu_only_aliases: bool,
    owns_targets: bool,
}

impl GfxBackend {
    pub(crate) fn prune_scene_uploads(&mut self) {
        self.scene_uploads
            .retain(|_, (owner, _)| owner.strong_count() != 0);
    }

    /// All replay attachment names must resolve to the original image with
    /// the recorded dimensions, format and sampling. Check before replacing
    /// any render commands so an unsupported frame can run on the guest path.
    pub fn matches_scene_targets(
        &self,
        frame: &ReplayFrame,
        targets: &HashMap<TargetRef, SurfaceId>,
    ) -> bool {
        self.matches_scene_targets_at(frame, targets, 1)
    }

    pub(crate) fn matches_scene_targets_at(
        &self,
        frame: &ReplayFrame,
        targets: &HashMap<TargetRef, SurfaceId>,
        scale: u32,
    ) -> bool {
        let count = usize::from(frame.colour.is_some())
            + usize::from(frame.depth.is_some())
            + frame.aux.len();
        let mut distinct = std::collections::HashSet::with_capacity(count);
        frame.ops.iter().all(|(_, op)| match op {
            ReplayOp::Draw { textures, .. } => textures.iter().flatten().all(|t| match t {
                ReplayTexture::Frozen(sample) => self.frozen_texture(sample, scale).is_some(),
                _ => true,
            }),
            _ => true,
        }) && targets.len() == count
            && targets.contains_key(&frame.presented)
            && targets.iter().all(|(&name, &id)| {
                let desc = match name {
                    TargetRef::SceneColour => frame.colour,
                    TargetRef::SceneDepth => frame.depth,
                    TargetRef::Aux(i) => frame.aux.get(usize::from(i)).copied(),
                    TargetRef::None => None,
                };
                distinct.insert(id)
                    && desc.is_some_and(|desc| self.scene_target_matches(id, desc, scale))
            })
    }

    pub(crate) fn read_scene_target(
        &mut self,
        target: SurfaceId,
    ) -> Result<([u32; 2], Vec<u8>), String> {
        let (width, height) = self
            .surface_extent(target)
            .ok_or("scene target lost before readback")?;
        let (mut pixels, order) = self.read_color(target)?;
        if order == gfx::encode::HostPixelOrder::Bgra8 {
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
        }
        Ok(([width, height], pixels))
    }

    /// Check before committing a scene to an existing physical target.
    pub(crate) fn scene_target_matches(&self, id: SurfaceId, desc: AuxTarget, scale: u32) -> bool {
        let Ok(surface) = self.store.get(id) else {
            return false;
        };
        let actual = surface.desc();
        desc.width.checked_mul(scale) == Some(actual.width)
            && desc.height.checked_mul(scale) == Some(actual.height)
            && D3DFormat::from_raw(desc.format) == Some(actual.format)
            && actual.samples == 1
            && (surface.kind() == gfx::surface::SurfaceKind::DepthStencil) == desc.depth
            && self.scales.get(&id).copied().unwrap_or(1) == scale
    }
}

impl<'a> GuestDrawPass<'a> {
    /// A private target draw. It submits no guest visibility feedback.
    pub fn new(gpu: &'a mut GfxBackend, frame: &ReplayFrame, scale: u32) -> Result<Self, String> {
        Self::start(gpu, frame, scale, false, None)
    }

    /// The window draw. Visibility reports belong only to this playback.
    pub fn new_display(
        gpu: &'a mut GfxBackend,
        frame: &ReplayFrame,
        scale: u32,
    ) -> Result<Self, String> {
        Self::start(gpu, frame, scale, true, None)
    }

    /// Draw on the renderer's existing targets at their common scale.
    /// Guest reads and the next view can use the contents directly.
    pub fn on_existing(
        gpu: &'a mut GfxBackend,
        frame: &ReplayFrame,
        targets: &HashMap<TargetRef, SurfaceId>,
    ) -> Result<Self, String> {
        let presented = targets
            .get(&frame.presented)
            .ok_or("missing existing present target")?;
        let scale = gpu.scales.get(presented).copied().unwrap_or(1);
        Self::start(gpu, frame, scale, true, Some(targets))
    }

    fn start(
        gpu: &'a mut GfxBackend,
        frame: &ReplayFrame,
        scale: u32,
        display: bool,
        existing: Option<&HashMap<TargetRef, SurfaceId>>,
    ) -> Result<Self, String> {
        if !(1..=crate::MAX_SCALE).contains(&scale) {
            return Err(format!(
                "scene scale {scale} is outside 1..={}",
                crate::MAX_SCALE
            ));
        }
        if !frame.refusals.is_empty() {
            return Err(format!(
                "scene cannot be replayed: {}",
                frame.refusals.join(", ")
            ));
        }
        if existing.is_none()
            && let Some(cause) = if display {
                frame.display_target_refusal()
            } else {
                frame.private_target_refusal()
            }
        {
            return Err(cause.into());
        }
        gpu.prune_scene_uploads();
        let old_count_samples = std::mem::replace(&mut gpu.count_samples, display);
        // Fresh scene targets use GPU-only aliases. Borrowed targets
        // retain the original resolver and its supported formats.
        let old_gpu_only_aliases = gpu.single_copy_aliases;
        if existing.is_none() {
            gpu.single_copy_aliases = true;
        }
        let lighting = gpu.scene_lighting();
        gpu.set_scene_receiver(false);
        let mut pass = Self {
            lighting,
            gpu,
            scale,
            targets: HashMap::new(),
            commands: Commands::default(),
            presented: frame.presented,
            display,
            old_count_samples,
            old_gpu_only_aliases,
            owns_targets: existing.is_none(),
        };
        if let Some(colour) = frame.colour {
            pass.resolve_target(TargetRef::SceneColour, colour, existing)?;
        }
        if let Some(depth) = frame.depth {
            pass.resolve_target(TargetRef::SceneDepth, depth, existing)?;
        }
        for (index, &aux) in frame.aux.iter().enumerate() {
            let id = u16::try_from(index).map_err(|_| "too many scene targets")?;
            pass.resolve_target(TargetRef::Aux(id), aux, existing)?;
        }
        pass.surface(frame.presented)?;
        if existing.is_none() {
            for (target, seed) in &frame.initial {
                match seed {
                    crate::scene_sink::TargetSeed::Colour(colour) => {
                        let surface = pass.surface(*target)?;
                        let clear = gfx::Clear {
                            color: Some(*colour),
                            ..gfx::Clear::NONE
                        };
                        pass.gpu.clear(surface, None, clear)?;
                    }
                    crate::scene_sink::TargetSeed::Upload { desc, bytes } => {
                        let surface = pass.surface(*target)?;
                        pass.gpu.write_scene_target(surface, desc, bytes, scale)?;
                    }
                }
            }
        }
        Ok(pass)
    }

    fn resolve_target(
        &mut self,
        target: TargetRef,
        desc: AuxTarget,
        existing: Option<&HashMap<TargetRef, SurfaceId>>,
    ) -> Result<(), String> {
        let limit = self.gpu.ctx.device.limits().max_texture_dimension_2d / self.scale;
        if desc.width == 0 || desc.height == 0 || desc.width > limit || desc.height > limit {
            return Err(format!(
                "scene target {target:?} has invalid extent {}x{} at {}x",
                desc.width, desc.height, self.scale
            ));
        }
        let format = D3DFormat::from_raw(desc.format)
            .ok_or_else(|| format!("unknown scene target format {}", desc.format))?;
        let surface = if let Some(existing) = existing {
            let &id = existing
                .get(&target)
                .ok_or_else(|| format!("missing existing target {target:?}"))?;
            if !self.gpu.scene_target_matches(id, desc, self.scale) {
                return Err(format!(
                    "existing target {target:?} does not match the scene"
                ));
            }
            id
        } else {
            <GfxBackend as ScaleView<GfxBackend>>::create_scaled_surface(
                self.gpu,
                format,
                desc.width,
                desc.height,
                desc.depth,
                self.scale,
            )?
        };
        self.targets.insert(target, surface);
        Ok(())
    }

    /// Resolves a scene attachment for a typed stage.
    pub fn surface(&self, target: TargetRef) -> Result<SurfaceId, String> {
        self.targets
            .get(&target)
            .copied()
            .ok_or_else(|| format!("missing scene target {target:?}"))
    }

    /// The GPU recording core used by both the typed and carried stages.
    pub fn backend_mut(&mut self) -> &mut GfxBackend {
        self.gpu
    }

    /// Applies an op in its slot. Merely changing slots does not close
    /// the pending pass; writes, copies and samples order themselves via
    /// the recording backend.
    pub fn apply(&mut self, op: &ReplayOp) -> Result<(), String> {
        self.commands
            .apply(self.gpu, &self.targets, self.display, self.scale, op)
    }

    /// Completes outstanding packets without waiting for the device.
    pub fn submit(&mut self) -> Result<(), String> {
        self.gpu.flush()
    }

    /// Reads a pass snapshot as tightly packed RGBA, in display pixels.
    pub fn read_target(&mut self, colour: TargetRef) -> Result<([u32; 2], Vec<u8>), String> {
        let target = self.surface(colour)?;
        self.gpu.read_scene_target(target)
    }

    /// Reads the completed private frame. Its attachments are released.
    pub fn read_rgba(mut self) -> Result<([u32; 2], Vec<u8>), String> {
        self.read_target(self.presented)
    }

    /// The graph samples its lighting parameter when the render thread
    /// takes the scene. Its display observer receives that same value.
    pub fn set_lighting(&mut self, lighting: crate::Lighting) {
        self.lighting = lighting;
    }

    /// Submits the frame and calls the backend's window hook without a
    /// guest Swap finish wait. Retain the shown target until the next
    /// display swap: callers may read the most recently presented surface.
    pub fn present(mut self, flags: u32, gamma: Option<&[u8; 768]>) -> Result<(), String> {
        self.publish(flags, gamma)
    }

    /// Submit and transfer the canonical frame's reports to its guest
    /// ticket table. A failed or abandoned draw retires them in Drop.
    pub fn present_with_outputs(
        mut self,
        flags: u32,
        gamma: Option<&[u8; 768]>,
    ) -> Result<crate::threaded::owner::Outputs<HostTexture>, String> {
        self.publish(flags, gamma)?;
        Ok(std::mem::take(&mut self.commands.outputs))
    }

    fn publish(&mut self, flags: u32, gamma: Option<&[u8; 768]>) -> Result<(), String> {
        if !self.display {
            return Err("a private scene capture cannot be presented".into());
        }
        let target = self.surface(self.presented)?;
        self.gpu
            .submit_present(None, target, flags, gamma, false, self.lighting)?;
        let retained = if self.owns_targets {
            self.targets.remove(&self.presented)
        } else {
            None
        };
        if let Some(old) = std::mem::replace(&mut self.gpu.scene_presented, retained) {
            self.gpu.scales.remove(&old);
            self.gpu.store.remove(old);
        }
        Ok(())
    }
}

fn drawn(outcome: DrawOutcome) -> Result<(), String> {
    match outcome {
        DrawOutcome::Drawn => Ok(()),
        DrawOutcome::Skipped(why) | DrawOutcome::Unsupported(why) => Err(why),
    }
}

impl Drop for GuestDrawPass<'_> {
    fn drop(&mut self) {
        // An error may abandon a partly recorded frame. Retire its
        // pending GPU uses before releasing target handles from the store.
        let _ = self.gpu.flush();
        for (_, ticket) in self.commands.outputs.tickets.drain(..) {
            self.gpu.drop_pixel_count(ticket);
        }
        if self.owns_targets {
            for (_, target) in self.targets.drain() {
                self.gpu.scales.remove(&target);
                self.gpu.store.remove(target);
            }
        }
        self.gpu
            .set_scene_receiver(self.gpu.scene_lighting() == crate::Lighting::E3);
        self.gpu.count_samples = self.old_count_samples;
        self.gpu.single_copy_aliases = self.old_gpu_only_aliases;
    }
}

impl SceneDevice for GfxBackend {
    fn record_scene_stage(
        &mut self,
        stage: &crate::SceneStage<TargetRef>,
        surfaces: crate::ExternalSurfaces<SurfaceId>,
    ) -> Result<(), String> {
        self.record_replay_stage(stage, surfaces)
    }

    fn scene_upload(&mut self, asset: &Arc<DecodedTexture>) -> Result<HostTexture, String> {
        let key = Arc::as_ptr(asset) as usize;
        if let Some((_, resident)) = self.scene_uploads.get(&key) {
            return Ok(resident.shared());
        }
        let resident = self.upload_texture(asset)?;
        self.scene_uploads
            .insert(key, (Arc::downgrade(asset), resident.shared()));
        Ok(resident)
    }

    fn frozen_texture(
        &self,
        sample: &crate::scene_sink::FrozenSample,
        scale: u32,
    ) -> Option<HostTexture> {
        let images = sample.images()?;
        let image = if scale == 1 {
            &images.reference
        } else {
            images.display.as_ref()?
        };
        image.for_device(&self.ctx.device)
    }
    fn share_scene_texture(t: &HostTexture) -> HostTexture {
        t.shared()
    }

    fn write_scene_target(
        &mut self,
        target: SurfaceId,
        desc: &TextureDesc,
        bytes: &[u8],
        scale: u32,
    ) -> Result<(), String> {
        if scale == 1 {
            return if self.write_surface(target, desc, bytes)? {
                Ok(())
            } else {
                Err("scene upload refused".into())
            };
        }
        // Guest uploads have the original pitch. Expand on the GPU.
        let info = *self.store.get(target).map_err(|e| e.to_string())?.desc();
        let stage = self.create_surface(
            info.format,
            info.width / scale,
            info.height / scale,
            info.format.is_depth(),
        )?;
        let result = (|| {
            if !self.write_surface(stage, desc, bytes)? {
                return Err("scene upload refused".into());
            }
            self.resize_image(stage, target, None)
        })();
        let release = self.destroy_surface(stage);
        result.and(release)
    }
}
