//! The maps a shadow caster call freezes for its receivers.
use native_render::shadow_gpu::SampledMap;
use std::sync::{Arc, Mutex};

/// The maps one caster call froze, by size: a receiver binds the map of
/// the size it records at.
#[derive(Default)]
pub struct Maps(Vec<SampledMap>);

impl Maps {
    pub fn of(map: Option<SampledMap>) -> Self {
        Self(map.into_iter().collect())
    }

    /// Replace the map of a size. A failed capture (`None`) leaves that size
    /// with none; `size` is the target size.
    pub fn put(&mut self, size: (u32, u32), map: Option<SampledMap>) {
        let size = map.as_ref().map_or(size, |m| m.size);
        self.0.retain(|m| m.size != size);
        self.0.extend(map);
    }

    /// The map whose texture is `size` texels.
    pub fn at(&self, size: (u32, u32)) -> Option<&SampledMap> {
        self.0.iter().find(|m| m.size == size)
    }

    /// The last map put.
    pub fn latest(&self) -> Option<&SampledMap> {
        self.0.last()
    }
}

/// A per-caster-call value, held by that call's receivers. A failed
/// capture leaves its size empty.
pub type Output = Arc<Mutex<Maps>>;

#[cfg(test)]
mod scale_tests {
    use super::*;
    use gfx::GfxContext;
    use native_render::shadow_gpu::MapKind;

    fn map(ctx: &GfxContext, size: (u32, u32)) -> SampledMap {
        let view = ctx
            .device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("scaled map"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        SampledMap {
            kind: MapKind::ColourMask,
            view,
            size,
        }
    }

    #[test]
    fn each_size_keeps_its_own_map() {
        let Ok(ctx) = GfxContext::headless() else {
            return;
        };
        let mut maps = Maps::default();
        maps.put((256, 256), Some(map(&ctx, (256, 256))));
        maps.put((512, 512), Some(map(&ctx, (512, 512))));
        assert_eq!(maps.at((256, 256)).unwrap().size, (256, 256));
        assert_eq!(maps.at((512, 512)).unwrap().size, (512, 512));
        assert_eq!(maps.latest().unwrap().size, (512, 512));
        assert!(maps.at((128, 128)).is_none());
        // A failed capture at one size leaves only that size without a map.
        maps.put((512, 512), None);
        assert!(maps.at((512, 512)).is_none());
        assert!(maps.at((256, 256)).is_some());
    }
}
