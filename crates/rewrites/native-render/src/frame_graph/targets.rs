//! The frame's targets, by [`TargetRef`], all at one [`RenderScale`]
//! (docs/graphics/high-resolution.md). The scene's colour and depth are
//! the game's 640x480 frame at the scale; nothing keeps a 1x copy. A
//! target the frame names that has no home here panics by name.

use frame_types::TargetRef;

use crate::render_scale::RenderScale;

/// The game's frame in its own pixels.
pub const GUEST: (u32, u32) = (640, 480);
pub const COLOUR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

/// One target's texture and its whole view.
#[derive(Clone, Debug)]
pub struct Target {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub format: wgpu::TextureFormat,
    pub size: (u32, u32),
}

impl Target {
    fn new(device: &wgpu::Device, label: &str, size: (u32, u32), format: wgpu::TextureFormat) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: size.0.max(1), height: size.1.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Self { texture, view, format, size }
    }
}

/// The targets one draw of a frame uses, kept across frames at the same
/// scale. The display has one set and a capture makes its own.
pub struct Targets {
    scale: RenderScale,
    colour: Option<Target>,
    depth: Option<Target>,
}

impl Targets {
    pub fn new(scale: RenderScale) -> Self {
        Self { scale, colour: None, depth: None }
    }

    pub fn scale(&self) -> RenderScale {
        self.scale
    }

    /// Moves to `scale`; the targets are made again at their next use.
    pub fn set_scale(&mut self, scale: RenderScale) {
        if scale != self.scale {
            *self = Self::new(scale);
        }
    }

    /// The scene's size in texels.
    pub fn size(&self) -> (u32, u32) {
        self.scale.size(GUEST.0, GUEST.1)
    }

    /// The target `r` names, made at its first use; `None` for
    /// [`TargetRef::None`].
    pub fn get(&mut self, device: &wgpu::Device, r: TargetRef) -> Option<Target> {
        let size = self.size();
        match r {
            TargetRef::None => None,
            TargetRef::SceneColour => Some(
                self.colour.get_or_insert_with(|| Target::new(device, "scene colour", size, COLOUR_FORMAT)).clone(),
            ),
            TargetRef::SceneDepth => {
                Some(self.depth.get_or_insert_with(|| Target::new(device, "scene depth", size, DEPTH_FORMAT)).clone())
            }
            TargetRef::Aux(n) => unimplemented!("targets: aux {n}, a frame target with no role in the frame graph"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scene_is_the_guest_frame_at_the_scale() {
        assert_eq!(Targets::new(RenderScale::ONE).size(), GUEST);
        assert_eq!(Targets::new(RenderScale::from(3)).size(), (1920, 1440));
        let wide = RenderScale::for_output(2560, 1440).unwrap();
        assert_eq!(Targets::new(wide).size(), (2560, 1440));
    }
}
