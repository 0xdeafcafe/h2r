//! The frame's targets, by [`TargetRef`], all at one [`RenderScale`]
//! (docs/graphics/high-resolution.md). The scene's colour and depth are
//! the game's 640x480 frame at the scale; nothing keeps a 1x copy. The
//! game's own targets are the entries of Halo's render-target table
//! ([`GAME`], docs/graphics/retire-d3d-display.md, "The graph's
//! targets"), each its guest size at the scale. A target the frame names
//! that has no home here panics by name.

use frame_types::TargetRef;

use crate::render_scale::RenderScale;

/// The game's frame in its own pixels.
pub const GUEST: (u32, u32) = (640, 480);
pub const COLOUR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;
/// A game depth target's format (target 9, the shadow map): its depth is
/// sampled as bits (`gfx::depth_bits`), which takes a float depth.
pub const GAME_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32FloatStencil8;

/// A game target's guest format, from `0x0001d6f0`'s format byte: linear
/// (`dl`) and depth (`cl`) pick it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameFormat {
    /// `D3DFMT_A8R8G8B8` (`0x06`), swizzled.
    Argb,
    /// `D3DFMT_LIN_A8R8G8B8` (`0x12`).
    LinearArgb,
    /// `D3DFMT_LIN_D24S8` (`0x2e`).
    LinearDepth,
}

impl GameFormat {
    /// The guest's `D3DFORMAT` code.
    pub const fn d3d(self) -> u32 {
        match self {
            GameFormat::Argb => 0x06,
            GameFormat::LinearArgb => 0x12,
            GameFormat::LinearDepth => 0x2e,
        }
    }

    pub const fn is_depth(self) -> bool {
        matches!(self, GameFormat::LinearDepth)
    }

    /// The frame graph's format for it.
    pub const fn native(self) -> wgpu::TextureFormat {
        if self.is_depth() { GAME_DEPTH_FORMAT } else { COLOUR_FORMAT }
    }
}

/// An entry of Halo's render-target table (`0x004b4b58`, stride `0x98`)
/// as `0x0001df20` makes it: its guest size (`+0x80`, `+0x84`), format
/// and levels (`+0x78`, each half the last).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GameTarget {
    pub index: u8,
    pub size: (u32, u32),
    pub format: GameFormat,
    pub levels: u8,
}

const fn entry(index: u8, w: u32, h: u32, format: GameFormat, levels: u8) -> GameTarget {
    GameTarget { index, size: (w, h), format, levels }
}

/// Every entry the game makes, by index. `0x0001d770` makes them at
/// boot, through `0x0001db10` (on the heap: 1, 9, `0xf`) and
/// `0x0001df20` (in entry 1's or entry 9's memory: the rest);
/// `0x0001dc40` makes `0x12` when a screen effect first wants it
/// (`0x00013bd2`, `0x0001df14`). No code makes 0, 2 to 4, `0xe`, `0x10`,
/// `0x11`, `0x14`, `0x18` or `0x1c` up.
pub const GAME: [GameTarget; 19] = [
    entry(0x01, 640, 480, GameFormat::LinearArgb, 1),
    entry(0x05, 320, 240, GameFormat::LinearArgb, 1),
    entry(0x06, 320, 240, GameFormat::LinearArgb, 1),
    entry(0x07, 160, 120, GameFormat::LinearArgb, 1),
    entry(0x08, 160, 120, GameFormat::LinearArgb, 1),
    entry(0x09, 512, 512, GameFormat::LinearDepth, 1),
    entry(0x0a, 128, 128, GameFormat::LinearArgb, 1),
    entry(0x0b, 128, 128, GameFormat::LinearArgb, 1),
    entry(0x0c, 128, 128, GameFormat::LinearArgb, 1),
    entry(0x0d, 256, 256, GameFormat::Argb, 3),
    entry(0x0f, 64, 64, GameFormat::LinearArgb, 1),
    entry(0x12, 640, 480, GameFormat::LinearArgb, 1),
    entry(0x13, 320, 240, GameFormat::LinearArgb, 1),
    entry(0x15, 512, 256, GameFormat::Argb, 1),
    entry(0x16, 512, 256, GameFormat::Argb, 1),
    entry(0x17, 320, 240, GameFormat::LinearArgb, 1),
    entry(0x19, 320, 240, GameFormat::LinearArgb, 1),
    entry(0x1a, 320, 240, GameFormat::LinearArgb, 1),
    entry(0x1b, 320, 240, GameFormat::LinearArgb, 1),
];

/// The table entry `n`, when the game makes it.
pub fn game(n: u8) -> Option<&'static GameTarget> {
    GAME.iter().find(|t| t.index == n)
}

/// The select's static surface `[0x00509384]` (index 9's colour): the
/// target select points it at entry 9's data and size
/// (`0x00014c61..0x00014c86`), so it is entry 9's size as
/// `D3DFMT_LIN_A8R8G8B8`.
pub const STATIC_SLOT: u32 = 0x0050_9384;

/// One target's texture and the view of its level.
#[derive(Clone, Debug)]
pub struct Target {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub format: wgpu::TextureFormat,
    /// The level's size in texels.
    pub size: (u32, u32),
    /// The mip level [`Target::view`] is of.
    pub level: u32,
}

impl Target {
    fn new(device: &wgpu::Device, label: &str, size: (u32, u32), format: wgpu::TextureFormat, levels: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: size.0.max(1), height: size.1.max(1), depth_or_array_layers: 1 },
            mip_level_count: levels.max(1),
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = level_view(&texture, 0);
        Self { texture, view, format, size, level: 0 }
    }

    /// The same texture's `level`.
    fn at_level(&self, level: u32) -> Self {
        Self {
            texture: self.texture.clone(),
            view: level_view(&self.texture, level),
            format: self.format,
            size: level_size(self.size, level),
            level,
        }
    }

    /// The level as a copy's source or destination.
    pub fn image_copy(&self) -> wgpu::TexelCopyTextureInfo<'_> {
        wgpu::TexelCopyTextureInfo {
            texture: &self.texture,
            mip_level: self.level,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        }
    }
}

fn level_view(texture: &wgpu::Texture, level: u32) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        base_mip_level: level,
        mip_level_count: Some(1),
        ..Default::default()
    })
}

/// Level `level` of a texture `size` at level 0.
pub fn level_size(size: (u32, u32), level: u32) -> (u32, u32) {
    ((size.0 >> level).max(1), (size.1 >> level).max(1))
}

/// The guest target `r` names: its guest size, format and levels; `None`
/// for the scene's own and [`TargetRef::None`]. A table entry the game
/// does not make, a level past its count, or a target with no home here
/// panics by name.
pub fn guest_target(r: TargetRef) -> Option<GameTarget> {
    let made = |n: u8| *game(n).unwrap_or_else(|| unimplemented!("targets: game target {n:#x}, which the game's table never makes"));
    match r {
        TargetRef::None | TargetRef::SceneColour | TargetRef::SceneDepth => None,
        TargetRef::Game(n) => Some(made(n)),
        TargetRef::GameLevel { target, level } => {
            let t = made(target);
            if level >= t.levels {
                unimplemented!("targets: level {level} of game target {target:#x}, which has {}", t.levels);
            }
            Some(t)
        }
        TargetRef::Slot(STATIC_SLOT) => Some(GameTarget { format: GameFormat::LinearArgb, ..made(9) }),
        TargetRef::Slot(g) => unimplemented!("targets: the slot target at {g:#x}, which has no home in the frame graph"),
        TargetRef::Aux(n) => unimplemented!("targets: aux {n}, a frame target with no role in the frame graph"),
    }
}

/// Where a game target's texture lives in [`Targets`]: a table entry
/// (all its levels) or a slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Home {
    Entry(u8),
    Slot(u32),
}

/// The targets one draw of a frame uses, kept across frames at the same
/// scale. The display has one set and a capture makes its own.
pub struct Targets {
    scale: RenderScale,
    colour: Option<Target>,
    depth: Option<Target>,
    game: std::collections::HashMap<Home, Target>,
    /// The depth a colour-only bind of a game target draws its stages
    /// with, by size.
    unbound_depth: std::collections::HashMap<(u32, u32), Target>,
}

impl Targets {
    pub fn new(scale: RenderScale) -> Self {
        Self { scale, colour: None, depth: None, game: Default::default(), unbound_depth: Default::default() }
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

    /// The level-0 size in texels of a game target of guest size `guest`.
    pub fn game_size(&self, guest: (u32, u32)) -> (u32, u32) {
        self.scale.size(guest.0, guest.1)
    }

    /// The target `r` names, made at its first use; `None` for
    /// [`TargetRef::None`].
    pub fn get(&mut self, device: &wgpu::Device, r: TargetRef) -> Option<Target> {
        let size = self.size();
        match r {
            TargetRef::None => None,
            TargetRef::SceneColour => Some(
                self.colour.get_or_insert_with(|| Target::new(device, "scene colour", size, COLOUR_FORMAT, 1)).clone(),
            ),
            TargetRef::SceneDepth => {
                Some(self.depth.get_or_insert_with(|| Target::new(device, "scene depth", size, DEPTH_FORMAT, 1)).clone())
            }
            TargetRef::Game(_) | TargetRef::GameLevel { .. } | TargetRef::Slot(_) | TargetRef::Aux(_) => {
                let shape = guest_target(r).expect("a game target");
                let (home, level) = match r {
                    TargetRef::GameLevel { target, level } => (Home::Entry(target), u32::from(level)),
                    TargetRef::Slot(g) => (Home::Slot(g), 0),
                    _ => (Home::Entry(shape.index), 0),
                };
                let size = self.game_size(shape.size);
                let t = self.game.entry(home).or_insert_with(|| {
                    let label = match home {
                        Home::Entry(n) => format!("game target {n:#x}"),
                        Home::Slot(g) => format!("game slot {g:#x}"),
                    };
                    Target::new(device, &label, size, shape.format.native(), u32::from(shape.levels))
                });
                Some(if level == 0 { t.clone() } else { t.at_level(level) })
            }
        }
    }

    /// The depth for a colour-only bind of `size` texels: the scene's
    /// at the scene's size, else one of that size.
    pub fn unbound_depth(&mut self, device: &wgpu::Device, size: (u32, u32)) -> Target {
        if size == self.size() {
            return self.get(device, TargetRef::SceneDepth).expect("scene depth");
        }
        self.unbound_depth.entry(size).or_insert_with(|| Target::new(device, "unbound depth", size, DEPTH_FORMAT, 1)).clone()
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

    #[test]
    fn the_table_is_what_the_boot_makes() {
        let made: Vec<u8> = (0..0x20).filter(|&n| game(n).is_some()).collect();
        assert_eq!(made, [1, 5, 6, 7, 8, 9, 0xa, 0xb, 0xc, 0xd, 0xf, 0x12, 0x13, 0x15, 0x16, 0x17, 0x19, 0x1a, 0x1b]);
        let shadow = game(9).unwrap();
        assert_eq!((shadow.size, shadow.format.d3d()), ((512, 512), 0x2e));
        assert_eq!(shadow.format.native(), wgpu::TextureFormat::Depth32FloatStencil8, "the freeze reads float depth");
        let gel = game(0xd).unwrap();
        assert_eq!((gel.size, gel.format.d3d(), gel.levels), ((256, 256), 0x06, 3));
        assert_eq!(game(0xa).unwrap().size, (128, 128));
        assert_eq!(game(0x15).unwrap().size, (512, 256));
        assert_eq!(game(0x16).unwrap().size, (512, 256));
        assert_eq!(game(7).unwrap().format.d3d(), 0x12);
        for t in &GAME {
            assert!(!t.format.is_depth() || t.index == 9, "only entry 9 is depth");
            assert_eq!(t.format.native(), if t.index == 9 { GAME_DEPTH_FORMAT } else { COLOUR_FORMAT });
        }
    }

    #[test]
    fn a_ref_names_its_entry_level_or_slot() {
        assert_eq!(guest_target(TargetRef::SceneColour), None);
        assert_eq!(guest_target(TargetRef::None), None);
        assert_eq!(guest_target(TargetRef::Game(10)).unwrap().size, (128, 128));
        assert_eq!(guest_target(TargetRef::GameLevel { target: 0xd, level: 2 }).unwrap().index, 0xd);
        let colour = guest_target(TargetRef::Slot(STATIC_SLOT)).unwrap();
        assert_eq!((colour.size, colour.format), ((512, 512), GameFormat::LinearArgb), "index 9's colour is entry 9's memory");
    }

    #[test]
    #[should_panic(expected = "game target 0x14, which the game's table never makes")]
    fn an_unmade_entry_is_named() {
        guest_target(TargetRef::Game(0x14));
    }

    #[test]
    #[should_panic(expected = "level 3 of game target 0xd, which has 3")]
    fn a_level_past_the_count_is_named() {
        guest_target(TargetRef::GameLevel { target: 0xd, level: 3 });
    }

    #[test]
    #[should_panic(expected = "the slot target at 0x509370")]
    fn another_slot_is_named() {
        guest_target(TargetRef::Slot(0x0050_9370));
    }

    #[test]
    fn game_targets_take_the_scale() {
        let three = Targets::new(RenderScale::from(3));
        assert_eq!(three.game_size(game(9).unwrap().size), (1536, 1536));
        assert_eq!(three.game_size(game(7).unwrap().size), (480, 360));
        let wide = Targets::new(RenderScale::for_output(2560, 1440).unwrap());
        assert_eq!(wide.game_size(game(5).unwrap().size), (1280, 720));
        assert_eq!(level_size(three.game_size(game(0xd).unwrap().size), 2), (192, 192));
        assert_eq!(level_size((3, 1), 2), (1, 1));
    }
}
