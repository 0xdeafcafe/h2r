//! Initial attachment pixels for a later, independent draw of a sealed
//! frame. The input is copied before the owner records the first write.

use std::collections::HashMap;

use frame_scene::TargetRef;
use gfx::D3DFormat;
use gfx::surface::{SurfaceId, SurfaceKind};

use super::GfxBackend;

/// A read-only copy of an attachment's base level on its original device.
#[derive(Debug)]
pub struct FrozenTarget {
    device: wgpu::Device,
    texture: wgpu::Texture,
    guest_format: D3DFormat,
    kind: SurfaceKind,
}

impl FrozenTarget {
    /// Physical dimensions, including the raster scale of the source.
    pub fn size(&self) -> [u32; 2] {
        [self.texture.width(), self.texture.height()]
    }
}

impl GfxBackend {
    /// Capture the selected incoming attachments in a single submission on
    /// the original queue. A private encoder on this device can immediately
    /// enqueue its restore. The guest and GPU do not wait for a readback.
    pub fn freeze_scene_targets(
        &mut self,
        targets: &[(TargetRef, SurfaceId)],
    ) -> Result<HashMap<TargetRef, FrozenTarget>, String> {
        if targets.is_empty() {
            return Ok(HashMap::new());
        }
        // Validate the whole request before taking the encoder or allocating
        // an image. Never substitute a resolved colour for an MSAA attachment.
        let mut sources = HashMap::with_capacity(targets.len());
        for &(name, id) in targets {
            let s = self.store.get(id).map_err(|e| e.to_string())?;
            let texture = s.texture();
            if s.desc().samples != 1
                || texture.sample_count() != 1
                || !texture.usage().contains(wgpu::TextureUsages::COPY_SRC)
                || texture.dimension() != wgpu::TextureDimension::D2
                || texture.depth_or_array_layers() != 1
            {
                return Err(format!(
                    "cannot copy the incoming scene attachment {name:?}"
                ));
            }
            if name == TargetRef::None
                || sources
                    .insert(name, (s.desc().format, s.kind(), texture.clone()))
                    .is_some()
            {
                return Err(format!("invalid incoming scene attachment {name:?}"));
            }
        }
        let (mut encoder, _) = self.take_open()?;
        let mut images = HashMap::with_capacity(sources.len());
        for (name, (guest_format, kind, source)) in sources {
            let texture = self.ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("sealed frame initial attachment"),
                size: source.size(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: source.format(),
                usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            // Texture-to-texture copies of combined depth/stencil formats
            // must copy the entire subresource and all of its aspects.
            encoder.copy_texture_to_texture(
                source.as_image_copy(),
                texture.as_image_copy(),
                source.size(),
            );
            images.insert(
                name,
                FrozenTarget {
                    device: self.ctx.device.clone(),
                    texture,
                    guest_format,
                    kind,
                },
            );
        }
        self.submit(encoder);
        Ok(images)
    }

    pub fn can_restore_scene_target(&self, id: SurfaceId, input: &FrozenTarget) -> bool {
        self.store.get(id).is_ok_and(|target| {
            let t = target.texture();
            self.ctx.device == input.device
                && target.desc().samples == 1
                && t.sample_count() == 1
                && t.usage().contains(wgpu::TextureUsages::COPY_DST)
                && target.desc().format == input.guest_format
                && target.kind() == input.kind
                && t.format() == input.texture.format()
                && t.size() == input.texture.size()
        })
    }

    /// Initialize a fresh attachment before drawing. Several restorations
    /// and subsequent rendering can share this encoder and submission.
    pub fn restore_scene_target(
        &mut self,
        id: SurfaceId,
        input: &FrozenTarget,
    ) -> Result<(), String> {
        if !self.can_restore_scene_target(id, input) {
            return Err("incoming scene attachment differs in device, layout or dimensions".into());
        }
        let target = self
            .store
            .get(id)
            .map_err(|e| e.to_string())?
            .texture()
            .clone();
        let (mut encoder, passes) = self.take_open()?;
        encoder.copy_texture_to_texture(
            input.texture.as_image_copy(),
            target.as_image_copy(),
            target.size(),
        );
        self.open = Some((encoder, passes));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Backend;

    #[test]
    fn empty_or_invalid_selections_submit_nothing_and_restore_requires_the_original_grid() {
        let Ok(ctx) = gfx::GfxContext::headless() else {
            return;
        };
        let mut gpu = GfxBackend::new(ctx);
        let fmt = D3DFormat::LinA8R8G8B8;
        let small = gpu.create_surface(fmt, 4, 4, false).unwrap();
        let large = gpu.create_surface(fmt, 8, 8, false).unwrap();
        let z = gpu.create_surface(D3DFormat::LinD16, 4, 4, true).unwrap();
        let before = gpu.batch_stats().submits;
        assert!(gpu.freeze_scene_targets(&[]).unwrap().is_empty());
        let duplicate = [
            (TargetRef::SceneColour, small),
            (TargetRef::SceneColour, large),
        ];
        assert!(gpu.freeze_scene_targets(&duplicate).is_err());
        assert_eq!(gpu.batch_stats().submits, before);

        let images = gpu
            .freeze_scene_targets(&[(TargetRef::SceneDepth, z), (TargetRef::SceneColour, small)])
            .unwrap();
        assert_eq!(gpu.batch_stats().submits, before + 1);
        let input = &images[&TargetRef::SceneColour];
        assert_eq!(input.size(), [4, 4]);
        assert!(gpu.can_restore_scene_target(small, input));
        assert!(gpu.restore_scene_target(large, input).is_err());
        assert!(gpu.restore_scene_target(z, input).is_err());
        assert_eq!(gpu.batch_stats().submits, before + 1);
        assert_eq!(gpu.readbacks(), 0);
    }
}
