//! The carried command stream is identical for private images and live
//! scaled attachments. The backend decides where a mixed-grid attachment
//! writes and where a later guest sample obtains that result.

use super::*;
use crate::backend::{ExternalSurfaces, SceneStage};
use crate::threaded::owner::Outputs;

pub(crate) trait SceneDevice: Backend {
    /// Does the physical copy select the same projection for typed and carried draws?
    fn can_replay_flare(&self, _draw: &crate::ResolvedDraw) -> bool { true }

    fn record_scene_stage(
        &mut self,
        stage: &SceneStage<TargetRef>,
        surfaces: ExternalSurfaces<Self::Surface>,
        private_lighting: Option<crate::Lighting>,
    ) -> Result<usize, String>;
    fn scene_upload(&mut self, asset: &Arc<DecodedTexture>) -> Result<Self::Texture, String>;
    fn share_scene_texture(t: &Self::Texture) -> Self::Texture;
    fn frozen_texture(
        &self,
        sample: &crate::scene_sink::FrozenSample,
        grid: RenderScale,
    ) -> Option<Self::Texture>;
    fn write_scene_target(
        &mut self,
        target: Self::Surface,
        desc: &TextureDesc,
        bytes: &[u8],
        grid: RenderScale,
    ) -> Result<(), String>;
}

pub(crate) struct Commands<B: SceneDevice> {
    /// Set only for isolated captures. The live attachments keep their display policy.
    pub(crate) private_lighting: Option<crate::Lighting>,
    relit_ranges: usize,
    bound: Option<(B::Surface, Option<B::Surface>)>,
    pub(crate) outputs: Outputs<B::Texture>,
}

impl<B: SceneDevice> Default for Commands<B> {
    fn default() -> Self {
        Self {
            private_lighting: None,
            relit_ranges: 0,
            bound: None,
            outputs: Outputs::default(),
        }
    }
}

impl<B: SceneDevice> Commands<B> {
    pub(crate) fn check_private_lightmaps(&self) -> Result<(), String> {
        if self.private_lighting.is_some_and(crate::Lighting::is_relight)
            && self.relit_ranges == 0
        {
            return Err("no native world range in this scene uses the installed lightmap".into());
        }
        Ok(())
    }

    /// The draw's typed replacement, prepared on this replay's own
    /// attachments with the draw's own resolved texture inputs. It is used
    /// only for the draw it was admitted for (an equal packet), on the
    /// attachments that draw is bound to, in a private capture only at 1x
    /// (a live display's backend answers for its own scale). Every copy is
    /// prepared before any is committed, so a refusal leaves nothing drawn
    /// and the caller draws the exact D3D packet.
    #[allow(clippy::too_many_arguments)]
    fn draw_external(
        &self,
        gpu: &mut B,
        targets: &HashMap<TargetRef, B::Surface>,
        display: bool,
        scale: RenderScale,
        bound: (B::Surface, Option<B::Surface>),
        draw: &Arc<crate::backend::ResolvedDraw>,
        external: &crate::scene_sink::RetainedExternal,
        handles: &[Option<B::Texture>; 4],
    ) -> bool {
        if !gpu.can_replay_flare(draw) {
            return false;
        }
        let Some(surfaces) = external.resolve(draw, targets, bound, display, scale) else {
            return false;
        };
        let Ok(Some(recordings)) = gpu.prepare_external(
            surfaces,
            external.request,
            handles.each_ref().map(Option::as_ref),
            &external.plan,
            crate::TargetCopy::Reference,
            Arc::clone(&external.pass),
        ) else {
            return false;
        };
        if !crate::device::complete_recordings(&external.plan, &recordings) {
            return false;
        }
        for recording in recordings {
            gpu.commit_external(recording);
        }
        true
    }

    pub(crate) fn apply(
        &mut self,
        gpu: &mut B,
        targets: &HashMap<TargetRef, B::Surface>,
        display: bool,
        scale: RenderScale,
        op: &ReplayOp,
    ) -> Result<(), String> {
        let surface = |r: &TargetRef| {
            targets
                .get(r)
                .copied()
                .ok_or_else(|| format!("missing scene target {r:?}"))
        };
        let attachments = |colour: &TargetRef, depth: &TargetRef| -> Result<_, String> {
            Ok((
                surface(colour)?,
                (*depth != TargetRef::None)
                    .then(|| surface(depth))
                    .transpose()?,
            ))
        };
        match op {
            ReplayOp::Stage(stage) => {
                let surfaces = stage
                    .surfaces
                    .try_map(|r| targets.get(r).copied())
                    .ok_or("missing scene stage target")?;
                self.relit_ranges += gpu.record_scene_stage(stage, surfaces, self.private_lighting)?;
            }
            ReplayOp::Bind { colour, depth } => self.bound = Some(attachments(colour, depth)?),
            ReplayOp::Draw { draw, textures, external, .. } => {
                let (colour, depth) = self.bound.ok_or("scene draw before target bind")?;
                let mut handles: [Option<B::Texture>; 4] = Default::default();
                for (dst, src) in handles.iter_mut().zip(textures) {
                    *dst = match src {
                        None => None,
                        Some(ReplayTexture::Upload(asset)) => Some(gpu.scene_upload(asset)?),
                        Some(ReplayTexture::Live { target, .. }) => {
                            let source = surface(target)?;
                            Some(gpu.surface_texture(source)?.ok_or("live scene colour cannot be sampled")?)
                        }
                        Some(ReplayTexture::Frozen(sample)) => {
                            Some(gpu.frozen_texture(sample, scale).ok_or(
                                "frozen sample unavailable for this device and display scale",
                            )?)
                        }
                        Some(ReplayTexture::Sample(id)) => Some(B::share_scene_texture(
                            self.outputs.samples.get(id).ok_or_else(|| {
                                format!("scene texture sample {id} used before it was produced")
                            })?,
                        )),
                        Some(ReplayTexture::Placeholder(desc)) => {
                            return Err(format!(
                                "missing uploaded scene texture {}x{} {:?}",
                                desc.width, desc.height, desc.format
                            ));
                        }
                    };
                }
                if let Some(external) = external
                    && self.draw_external(gpu, targets, display, scale, (colour, depth), draw, external, &handles)
                {
                    return Ok(());
                }
                drawn(gpu.draw(colour, depth, draw, handles.each_ref().map(Option::as_ref))?)?;
            }
            ReplayOp::Clear {
                colour,
                depth,
                clear,
            } => {
                let (c, d) = attachments(colour, depth)?;
                gpu.clear(c, d, *clear)?;
            }
            ReplayOp::ClearRects {
                colour,
                depth,
                clear,
            } => {
                let (c, d) = attachments(colour, depth)?;
                drawn(gpu.clear_rects(c, d, clear)?)?;
            }
            ReplayOp::Copy { from, to } => {
                let (from, to) = (surface(from)?, surface(to)?);
                if from != to && !gpu.copy_surface(from, to)? {
                    return Err("scene target copy refused".into());
                }
            }
            ReplayOp::Upload { to, desc, bytes } => {
                gpu.write_scene_target(surface(to)?, desc, bytes, scale)?
            }
            ReplayOp::Sample { of, id, how } => {
                let of = surface(of)?;
                let t = match how {
                    ReplaySample::Snapshot { opaque } => gpu.snapshot_surface(of, *opaque)?,
                    ReplaySample::InPlace => gpu.surface_texture(of)?,
                    ReplaySample::Depth => gpu.depth_level_texture(of)?,
                    ReplaySample::Alias {
                        format,
                        width,
                        height,
                        pitch,
                        generation,
                        offset,
                        desc,
                        options,
                    } => gpu.depth_alias_texture(
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
                }
                .ok_or_else(|| format!("could not produce scene sample {id}"))?;
                if self.outputs.samples.insert(*id, t).is_some() {
                    return Err(format!("duplicate scene sample {id}"));
                }
            }
            ReplayOp::BeginFlareQuery if display => gpu.clear_pixel_count()?,
            ReplayOp::EndFlareQuery(proxy) if display => {
                let ticket = gpu
                    .report_pixel_count()?
                    .ok_or("native flare counter unavailable")?;
                self.outputs.tickets.push((*proxy, ticket));
            }
            ReplayOp::BeginFlareQuery | ReplayOp::EndFlareQuery(_) | ReplayOp::ObjectShadow(_) => {}
        }
        Ok(())
    }
}
