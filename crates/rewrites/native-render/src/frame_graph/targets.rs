//! The frame's targets, by [`TargetRef`], all at one [`RenderScale`]
//! (docs/graphics/high-resolution.md). The scene's colour and depth are
//! the game's 640x480 frame at the scale; a game target
//! ([`TargetRef::Game`]) is one surface per role at its guest size times
//! the scale, made again when its shape changes; nothing keeps a 1x copy.
//! A target the frame names that has no home here panics by name.

use std::collections::HashMap;

use frame_types::{GameTarget, TargetRef, TargetRole};

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
    /// Each game role's surface and the shape it was made for.
    games: HashMap<TargetRole, (GameTarget, Target)>,
    /// The depth a colour-only bind of a game target draws its stages
    /// with, by size.
    unbound_depth: HashMap<(u32, u32), Target>,
}

impl Targets {
    pub fn new(scale: RenderScale) -> Self {
        Self { scale, colour: None, depth: None, games: HashMap::new(), unbound_depth: HashMap::new() }
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
            TargetRef::Game(g) => Some(self.game(device, g)),
            TargetRef::Aux(n) => unimplemented!("targets: aux {n}, a frame target with no role in the frame graph"),
        }
    }

    /// `g`'s role's surface: the one kept while its shape holds, else a
    /// new one at `g`'s size times the scale.
    fn game(&mut self, device: &wgpu::Device, g: GameTarget) -> Target {
        let size = game_size(self.scale, g);
        if let Some((held, t)) = self.games.get(&g.role)
            && *held == g
        {
            return t.clone();
        }
        let format = if g.depth { DEPTH_FORMAT } else { COLOUR_FORMAT };
        let t = Target::new(device, &format!("game target {:?}", g.role), size, format);
        self.games.insert(g.role, (g, t.clone()));
        t
    }

    /// The depth a stage on a colour-only bind of a target of `size`
    /// (texels) is given: the scene's for the scene's size, else one of
    /// that size the graph keeps.
    pub fn unbound_depth(&mut self, device: &wgpu::Device, size: (u32, u32)) -> Target {
        if size == self.size() {
            return self.get(device, TargetRef::SceneDepth).expect("scene depth");
        }
        self.unbound_depth.entry(size).or_insert_with(|| Target::new(device, "unbound depth", size, DEPTH_FORMAT)).clone()
    }
}

/// A game target's size in texels at `scale`.
pub fn game_size(scale: RenderScale, g: GameTarget) -> (u32, u32) {
    scale.size(g.size.0, g.size.1)
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

    #[test]
    fn a_game_target_is_its_guest_size_at_the_scale() {
        let g = GameTarget { role: TargetRole::Game(0x15), size: (512, 256), depth: false };
        assert_eq!(game_size(RenderScale::ONE, g), (512, 256));
        assert_eq!(game_size(RenderScale::from(2), g), (1024, 512));
    }
}
