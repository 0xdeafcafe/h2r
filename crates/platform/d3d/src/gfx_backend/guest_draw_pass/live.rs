//! Confirm that an earlier in-place view still names the allocation now
//! written under its source target. The saved view proves identity only.
//! On replay the draw obtains a new view of that playback's attachment.

use super::*;
use crate::scene_sink::FrozenSample;
use gfx::surface::SurfaceKind;

pub(crate) fn live_draws_match(
    frame: &ReplayFrame,
    mut matches: impl FnMut(TargetRef, TargetRef, &FrozenSample) -> bool,
) -> bool {
    let mut colour = TargetRef::None;
    let mut depth = TargetRef::None;
    for (_, op) in &frame.ops {
        match op {
            ReplayOp::Bind {
                colour: c,
                depth: z,
            } => (colour, depth) = (*c, *z),
            ReplayOp::Draw { textures, .. } => {
                for t in textures.iter().flatten() {
                    if let ReplayTexture::Live { target, binding } = t
                        && (colour == TargetRef::None
                            || *target == colour
                            || *target == depth
                            || !matches(*target, colour, binding))
                    {
                        return false;
                    }
                }
            }
            _ => {}
        }
    }
    true
}

impl GfxBackend {
    /// Only single-level, sampleable colour attachments can be rebuilt by
    /// these frame targets. Two different guest names can alias one image.
    pub(crate) fn live_view_matches(
        &self,
        id: SurfaceId,
        colour: SurfaceId,
        desc: AuxTarget,
        scale: u32,
        bound: &HostTexture,
    ) -> bool {
        let (Ok(source), Ok(destination)) = (self.store.get(id), self.store.get(colour)) else {
            return false;
        };
        let texture = source.texture();
        source.kind() == SurfaceKind::Color
            && self.scene_target_matches(id, desc, scale)
            && texture.mip_level_count() == 1
            && texture.dimension() == wgpu::TextureDimension::D2
            && texture.depth_or_array_layers() == 1
            && texture
                .usage()
                .contains(wgpu::TextureUsages::TEXTURE_BINDING)
            && texture != destination.texture()
            && texture == &bound.texture
            && Some(source.view()) == bound.view.as_ref()
    }

    pub(crate) fn raw_live_views_match(
        &self,
        frame: &ReplayFrame,
        targets: &HashMap<TargetRef, SurfaceId>,
    ) -> bool {
        live_draws_match(frame, |source, colour, binding| {
            let (Some(&id), Some(&dest), Some(desc), Some(images)) = (
                targets.get(&source),
                targets.get(&colour),
                frame.desc(source),
                binding.images(),
            ) else {
                return false;
            };
            let scale = self.scales.get(&id).copied().unwrap_or(1);
            images.reference.device == self.ctx.device
                && self.live_view_matches(id, dest, desc, scale, &images.reference.texture)
        })
    }
}
