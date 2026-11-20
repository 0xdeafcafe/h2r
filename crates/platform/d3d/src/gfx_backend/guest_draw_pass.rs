//! The frame owner's guest draws. Each playback has its own attachments
//! and sampled-target versions. Draw preparation, upload rings and the
//! encoder are the same ones the renderer uses for typed scene stages.

mod typed;

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use frame_scene::{AuxTarget, TargetRef};
use gfx::surface::SurfaceId;
use gfx::texture::DecodedTexture;
use gfx::{D3DFormat, TextureDesc};

use super::{GfxBackend, HostTexture};
use crate::backend::{Backend, DrawOutcome};
use crate::scaled::ScaleView;
use crate::scene_sink::{FlareTickets, ReplayFrame, ReplayOp, ReplaySample, ReplayTexture};

/// A frame keeps uploads alive by Arc. Sharing the allocation identifies
/// the exact decoded texels without hashing guest assets at every draw.
pub(super) type TextureCache = HashMap<usize, (Weak<DecodedTexture>, HostTexture)>;

/// Records one sealed frame at display scale. Private captures and window
/// frames have disjoint render targets even when they use the same device.
pub struct GuestDrawPass<'a> {
    gpu: &'a mut GfxBackend,
    scale: u32,
    targets: HashMap<TargetRef, SurfaceId>,
    samples: HashMap<u32, HostTexture>,
    bound: Option<(SurfaceId, Option<SurfaceId>)>,
    presented: TargetRef,
    display: bool,
    old_count_samples: bool,
    old_gpu_only_aliases: bool,
    owns_targets: bool,
    flare_tickets: FlareTickets,
}

impl GfxBackend {
    /// All replay attachment names must resolve to the original image with
    /// the recorded dimensions, format and sampling. Check before replacing
    /// any render commands so an unsupported frame can run on the guest path.
    pub fn matches_scene_targets(&self, frame: &ReplayFrame, targets: &HashMap<TargetRef, SurfaceId>) -> bool {
        self.matches_scene_targets_at(frame, targets, 1)
    }

    pub(crate) fn matches_scene_targets_at(
        &self, frame: &ReplayFrame, targets: &HashMap<TargetRef, SurfaceId>, scale: u32,
    ) -> bool {
        let count = usize::from(frame.colour.is_some()) + usize::from(frame.depth.is_some()) + frame.aux.len();
        let mut distinct = std::collections::HashSet::with_capacity(count);
        targets.len() == count && targets.contains_key(&frame.presented) && targets.iter().all(|(&name, &id)| {
            let desc = match name {
                TargetRef::SceneColour => frame.colour,
                TargetRef::SceneDepth => frame.depth,
                TargetRef::Aux(i) => frame.aux.get(usize::from(i)).copied(),
                TargetRef::None => None,
            };
            distinct.insert(id) && desc.is_some_and(|desc| self.scene_target_matches(id, desc, scale))
        })
    }

    /// Check before committing a scene to an existing physical target.
    fn scene_target_matches(&self, id: SurfaceId, desc: AuxTarget, scale: u32) -> bool {
        let Ok(surface) = self.store.get(id) else { return false };
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
    pub fn new_display(gpu: &'a mut GfxBackend, frame: &ReplayFrame, scale: u32) -> Result<Self, String> {
        Self::start(gpu, frame, scale, true, None)
    }

    /// Draw on the renderer's existing targets at their common scale.
    /// Guest reads and the next view can use the contents directly.
    pub fn on_existing(
        gpu: &'a mut GfxBackend,
        frame: &ReplayFrame,
        targets: &HashMap<TargetRef, SurfaceId>,
    ) -> Result<Self, String> {
        let presented = targets.get(&frame.presented).ok_or("missing existing present target")?;
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
            return Err(format!("scene scale {scale} is outside 1..={}", crate::MAX_SCALE));
        }
        if !frame.refusals.is_empty() {
            return Err(format!("scene cannot be replayed: {}", frame.refusals.join(", ")));
        }
        if existing.is_none() && let Some(cause) = if display {
            frame.display_target_refusal()
        } else {
            frame.private_target_refusal()
        } {
            return Err(cause.into());
        }
        gpu.scene_uploads.retain(|_, (owner, _)| owner.strong_count() != 0);
        let old_count_samples = std::mem::replace(&mut gpu.count_samples, display);
        // Scene aliases must stay GPU operations, including aliased
        // formats that don't take the dedicated aligned depth-word pass.
        let old_gpu_only_aliases = std::mem::replace(&mut gpu.single_copy_aliases, true);
        let mut pass = Self {
            gpu,
            scale,
            targets: HashMap::new(),
            samples: HashMap::new(),
            bound: None,
            presented: frame.presented,
            display,
            old_count_samples,
            old_gpu_only_aliases,
            owns_targets: existing.is_none(),
            flare_tickets: Vec::new(),
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
        let format =
            D3DFormat::from_raw(desc.format).ok_or_else(|| format!("unknown scene target format {}", desc.format))?;
        let surface = if let Some(existing) = existing {
            let &id = existing
                .get(&target)
                .ok_or_else(|| format!("missing existing target {target:?}"))?;
            if !self.gpu.scene_target_matches(id, desc, self.scale) {
                return Err(format!("existing target {target:?} does not match the scene"));
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

    fn attachments(&self, colour: TargetRef, depth: TargetRef) -> Result<(SurfaceId, Option<SurfaceId>), String> {
        let colour = self.surface(colour)?;
        let depth = if depth == TargetRef::None {
            None
        } else {
            Some(self.surface(depth)?)
        };
        Ok((colour, depth))
    }

    /// The GPU recording core used by both the typed and carried stages.
    pub fn backend_mut(&mut self) -> &mut GfxBackend {
        self.gpu
    }

    /// Applies an op in its slot. Merely changing slots does not close
    /// the pending pass; writes, copies and samples order themselves via
    /// the recording backend.
    pub fn apply(&mut self, op: &ReplayOp) -> Result<(), String> {
        match op {
            ReplayOp::Stage(stage) => self.record_stage(stage)?,
            ReplayOp::Bind { colour, depth } => self.bound = Some(self.attachments(*colour, *depth)?),
            ReplayOp::Draw { draw, textures, .. } => {
                let (colour, depth) = self.bound.ok_or("scene draw before target bind")?;
                let mut handles: [Option<HostTexture>; 4] = Default::default();
                for (handle, texture) in handles.iter_mut().zip(textures) {
                    if let Some(texture) = texture {
                        *handle = Some(self.texture(texture)?);
                    }
                }
                let outcome = self
                    .gpu
                    .draw(colour, depth, draw, handles.each_ref().map(Option::as_ref))?;
                drawn(outcome)?;
            }
            ReplayOp::Clear { colour, depth, clear } => {
                let (colour, depth) = self.attachments(*colour, *depth)?;
                self.gpu.clear(colour, depth, *clear)?;
            }
            ReplayOp::ClearRects { colour, depth, clear } => {
                let (colour, depth) = self.attachments(*colour, *depth)?;
                drawn(self.gpu.clear_rects(colour, depth, clear)?)?;
            }
            ReplayOp::Copy { from, to } => {
                let (from, to) = (self.surface(*from)?, self.surface(*to)?);
                if from != to && !self.gpu.copy_surface(from, to)? {
                    return Err("scene target copy refused".into());
                }
            }
            ReplayOp::Upload { to, desc, bytes } => self.upload(*to, desc, bytes)?,
            ReplayOp::Sample { of, id, how } => {
                let of = self.surface(*of)?;
                let texture = match how {
                    ReplaySample::Snapshot { opaque } => self.gpu.snapshot_surface(of, *opaque)?,
                    ReplaySample::InPlace => self.gpu.surface_texture(of)?,
                    ReplaySample::Depth => self.gpu.depth_level_texture(of)?,
                    ReplaySample::Alias {
                        format,
                        width,
                        height,
                        pitch,
                        generation,
                        offset,
                        desc,
                        options,
                    } => self.gpu.depth_alias_texture(
                        of,
                        *format,
                        *width,
                        *height,
                        *pitch,
                        *generation,
                        *offset,
                        desc,
                        *options,
                    )?,
                };
                let texture = texture.ok_or_else(|| format!("could not produce scene sample {id}"))?;
                if self.samples.insert(*id, texture).is_some() {
                    return Err(format!("duplicate scene sample {id}"));
                }
            }
            ReplayOp::BeginFlareQuery => {
                if self.display {
                    self.gpu.clear_pixel_count()?;
                }
            }
            ReplayOp::EndFlareQuery(proxy) => {
                if self.display {
                    let ticket = self.gpu.report_pixel_count()?.ok_or("native flare counter unavailable")?;
                    self.flare_tickets.push((*proxy, ticket));
                }
            }
        }
        Ok(())
    }

    fn texture(&mut self, texture: &ReplayTexture) -> Result<HostTexture, String> {
        match texture {
            ReplayTexture::Upload(asset) => {
                let key = Arc::as_ptr(asset) as usize;
                if let Some((_, resident)) = self.gpu.scene_uploads.get(&key) {
                    return Ok(resident.shared());
                }
                let resident = self.gpu.upload_texture(asset)?;
                self.gpu
                    .scene_uploads
                    .insert(key, (Arc::downgrade(asset), resident.shared()));
                Ok(resident)
            }
            ReplayTexture::Sample(id) => self
                .samples
                .get(id)
                .map(HostTexture::shared)
                .ok_or_else(|| format!("scene texture sample {id} used before it was produced")),
            ReplayTexture::Placeholder(desc) => Err(format!(
                "missing uploaded scene texture {}x{} {:?}",
                desc.width, desc.height, desc.format
            )),
        }
    }

    fn upload(&mut self, to: TargetRef, desc: &TextureDesc, bytes: &[u8]) -> Result<(), String> {
        let target = self.surface(to)?;
        if self.scale == 1 {
            return if self.gpu.write_surface(target, desc, bytes)? {
                Ok(())
            } else {
                Err("scene upload refused".into())
            };
        }
        // Upload guest texels with their own pitch/format at their own size,
        // then expand on the GPU, including depth/stencil. The bytes never
        // have a scaled layout.
        let info = *self.gpu.store.get(target).map_err(|e| e.to_string())?.desc();
        let stage = self.gpu.create_surface(
            info.format,
            info.width / self.scale,
            info.height / self.scale,
            info.format.is_depth(),
        )?;
        let result = (|| {
            if !self.gpu.write_surface(stage, desc, bytes)? {
                return Err("scene upload refused".into());
            }
            self.gpu.resize_image(stage, target, None)
        })();
        let release = self.gpu.destroy_surface(stage);
        result.and(release)
    }

    /// Completes outstanding packets without waiting for the device.
    pub fn submit(&mut self) -> Result<(), String> {
        self.gpu.flush()
    }

    /// Reads a pass snapshot as tightly packed RGBA, in display pixels.
    pub fn read_target(&mut self, colour: TargetRef) -> Result<([u32; 2], Vec<u8>), String> {
        let target = self.surface(colour)?;
        let (width, height) = self
            .gpu
            .surface_extent(target)
            .ok_or("scene target lost before readback")?;
        let (mut pixels, order) = self.gpu.read_color(target)?;
        if order == gfx::encode::HostPixelOrder::Bgra8 {
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
        }
        Ok(([width, height], pixels))
    }

    /// Reads the completed private frame. Its attachments are released.
    pub fn read_rgba(mut self) -> Result<([u32; 2], Vec<u8>), String> {
        self.read_target(self.presented)
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
        mut self, flags: u32, gamma: Option<&[u8; 768]>,
    ) -> Result<crate::threaded::owner::Outputs<HostTexture>, String> {
        self.publish(flags, gamma)?;
        Ok(crate::threaded::owner::Outputs {
            tickets: std::mem::take(&mut self.flare_tickets),
            samples: std::mem::take(&mut self.samples),
        })
    }

    fn publish(&mut self, flags: u32, gamma: Option<&[u8; 768]>) -> Result<(), String> {
        if !self.display {
            return Err("a private scene capture cannot be presented".into());
        }
        let target = self.surface(self.presented)?;
        self.gpu.submit_present(None, target, flags, gamma, false)?;
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
        for (_, ticket) in self.flare_tickets.drain(..) {
            self.gpu.drop_pixel_count(ticket);
        }
        if self.owns_targets {
            for (_, target) in self.targets.drain() {
                self.gpu.scales.remove(&target);
                self.gpu.store.remove(target);
            }
        }
        self.gpu.count_samples = self.old_count_samples;
        self.gpu.single_copy_aliases = self.old_gpu_only_aliases;
    }
}
