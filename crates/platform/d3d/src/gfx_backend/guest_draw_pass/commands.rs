//! The carried command stream is identical for private images and live
//! scaled attachments. The backend decides where a mixed-grid attachment
//! writes and where a later guest sample obtains that result.

use super::*;
use crate::backend::{ExternalSurfaces, SceneStage};
use crate::threaded::owner::Outputs;

pub(crate) trait SceneDevice: Backend {
    fn record_scene_stage(
        &mut self,
        stage: &SceneStage<TargetRef>,
        surfaces: ExternalSurfaces<Self::Surface>,
    ) -> Result<(), String>;
    fn scene_upload(&mut self, asset: &Arc<DecodedTexture>) -> Result<Self::Texture, String>;
    fn share_scene_texture(t: &Self::Texture) -> Self::Texture;
    fn frozen_texture(
        &self,
        sample: &crate::scene_sink::FrozenSample,
        scale: u32,
    ) -> Option<Self::Texture>;
    fn write_scene_target(
        &mut self,
        target: Self::Surface,
        desc: &TextureDesc,
        bytes: &[u8],
        scale: u32,
    ) -> Result<(), String>;
}

pub(crate) struct Commands<B: SceneDevice> {
    bound: Option<(B::Surface, Option<B::Surface>)>,
    pub(crate) outputs: Outputs<B::Texture>,
}

impl<B: SceneDevice> Default for Commands<B> {
    fn default() -> Self {
        Self {
            bound: None,
            outputs: Outputs::default(),
        }
    }
}

impl<B: SceneDevice> Commands<B> {
    pub(crate) fn apply(
        &mut self,
        gpu: &mut B,
        targets: &HashMap<TargetRef, B::Surface>,
        display: bool,
        scale: u32,
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
                gpu.record_scene_stage(stage, surfaces)?;
            }
            ReplayOp::Bind { colour, depth } => self.bound = Some(attachments(colour, depth)?),
            ReplayOp::Draw { draw, textures, .. } => {
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
            ReplayOp::BeginFlareQuery | ReplayOp::EndFlareQuery(_) => {}
        }
        Ok(())
    }
}
