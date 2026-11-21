//! The viewport's world on the native renderer (docs/editor/polish-plan.md,
//! "The viewport on the native renderer", step 1; slice P14): one BSP
//! loaded with `native_render::load::load_world`, its base maps and
//! lightmap pages decoded by the renderer's own decodes, uploaded with
//! `GpuWorld::new` and drawn with the editor's camera into the viewport's
//! colour and depth targets. The look is the renderer's `world.wgsl`: base
//! map times lightmap. The Lighting menu's modes swap the images the
//! world is uploaded with ([`Lighting`]).
use crate::geometry::Camera;
use assets::ResourceFiles;
use map::{Map, TagId};
use native_render::camera::RenderCamera;
use native_render::cull::CullStats;
use native_render::gpu::{DepthRule, GpuWorld, Image, PassViewport};
use native_render::load;
use native_render::world::{LightmapKey, World, keep_draws};
use std::collections::HashMap;
use std::io::{Read, Seek};

/// What the viewport's world shows (the Lighting menu).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Lighting {
    /// The native renderer's picture: base map times lightmap.
    #[default]
    Game,
    /// Base maps at full brightness, no lightmap: geometry in dark places.
    Flat,
    /// The lightmap on white: the lighting alone.
    LightmapOnly,
    /// Flat at a third of its strength, for the collision layers on top.
    Collision,
}

impl Lighting {
    pub const ALL: [Self; 4] = [Self::Game, Self::Flat, Self::LightmapOnly, Self::Collision];

    /// The menu's name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Game => "Game",
            Self::Flat => "Flat",
            Self::LightmapOnly => "Lightmap only",
            Self::Collision => "Collision",
        }
    }

    /// The id the socket API and the saved layout use.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Game => "game",
            Self::Flat => "flat",
            Self::LightmapOnly => "lightmap",
            Self::Collision => "collision",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.id() == id)
    }

    /// A material's base map as this mode draws it.
    pub fn base(self, image: &Image) -> Image {
        match self {
            Self::Game | Self::Flat => image.clone(),
            Self::LightmapOnly => Image::solid([255; 4]),
            Self::Collision => scaled(image, 1, 3),
        }
    }

    /// A lightmap page as this mode draws it; `None` draws white.
    pub fn page(self, image: &Image) -> Option<Image> {
        match self {
            Self::Game | Self::LightmapOnly => Some(image.clone()),
            Self::Flat | Self::Collision => None,
        }
    }
}

/// `image` with its colour (not alpha) times `num / den`, every level.
/// Its BC blocks are dropped: they no longer match the pixels.
fn scaled(image: &Image, num: u16, den: u16) -> Image {
    let scale = |rgba: &[u8]| -> Vec<u8> {
        rgba.chunks_exact(4)
            .flat_map(|p| {
                let c = |v: u8| (u16::from(v) * num / den) as u8;
                [c(p[0]), c(p[1]), c(p[2]), p[3]]
            })
            .collect()
    };
    Image {
        width: image.width,
        height: image.height,
        rgba: scale(&image.rgba),
        mips: image.mips.iter().map(|m| scale(m)).collect(),
        blocks: None,
    }
}

/// The depth rule the editor's world pass writes: view depth over the far
/// plane, unfloored (the viewport's depth is `Depth32Float`). The overlay
/// pass that draws after the world writes the same depth
/// ([`depth_of`]), so its depth test agrees with the picture.
pub const DEPTH_RULE: DepthRule = DepthRule {
    full_scale: native_render::camera::D24_SCALE,
    floor: false,
};

/// The depth [`DEPTH_RULE`] stores for world point `p` seen by `camera`.
pub fn depth_of(camera: &RenderCamera, p: [f32; 3]) -> f32 {
    (camera.depth_of(p) / camera.far).clamp(0.0, 1.0)
}

/// The editor's camera as the renderer's, for a `size` (width, height)
/// target. Its view-projection is the editor's ([`Camera::view_proj`]).
pub fn render_camera(camera: &Camera, [width, height]: [u32; 2]) -> RenderCamera {
    let [forward, _, up] = camera.basis();
    let (near, far) = camera.near_far();
    let side = |v: u32| v.clamp(1, i16::MAX as u32) as i16;
    RenderCamera {
        position: camera.eye(),
        forward,
        up,
        fov: camera.fov_y,
        projection_xy: None,
        viewport: [0, 0, side(height), side(width)],
        near,
        far,
    }
}

/// A map's raw resources for the loader.
struct Files<'a>(&'a mut ResourceFiles);

impl tags::geometry::RawSource for Files<'_> {
    fn read_raw(&mut self, pointer: map::RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        self.0
            .read(pointer.file, pointer.offset, size)
            .map_err(|_| tags::Error::MissingResource(pointer.file))
    }
}

/// What the viewport's world counts ([`NativeLoad::stats`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NativeStats {
    pub draws: usize,
    pub batches: usize,
    pub materials: usize,
    pub lightmap_pages: usize,
    /// Draws left out: no base map decoded, or a lit draw whose page did
    /// not decode.
    pub untextured: usize,
    /// Sections that did not decode.
    pub failed_sections: u32,
}

/// One BSP's world and its decoded images, on the CPU: uploaded again
/// when the Lighting mode changes.
#[derive(Clone, Debug)]
pub struct NativeLoad {
    pub bsp: TagId,
    world: World,
    base: HashMap<u16, Image>,
    pages: HashMap<LightmapKey, Image>,
    stats: NativeStats,
    /// Why the lightmap did not read, when it did not.
    pub lightmap_error: Option<String>,
}

impl NativeLoad {
    /// Load BSP `index` of the scenario with its base maps and pages.
    pub fn read<R: Read + Seek>(
        map: &mut Map<R>,
        files: &mut ResourceFiles,
        index: usize,
    ) -> Result<Self, String> {
        let loaded = load::load_world(map, &mut Files(files), index).map_err(|e| e.to_string())?;
        let mut base = HashMap::new();
        for m in loaded.world.materials() {
            let image = loaded
                .shaders
                .get(usize::from(m))
                .and_then(|&shader| load::base_map(map, shader))
                .and_then(|bitmap| load::decode_first(map, files, bitmap));
            if let Some(image) = image {
                base.insert(m, image);
            }
        }
        let bitmap = loaded
            .lightmap_bitmap
            .and_then(|id| tags::DecodedTag::read(map, id).ok())
            .and_then(|tag| match tag.data {
                tags::TagData::Bitmap(b) => Some(b),
                _ => None,
            });
        let mut pages = HashMap::new();
        for k in loaded.world.lightmap_keys() {
            let image = bitmap
                .as_ref()
                .and_then(|b| b.images.get(usize::from(k.page)))
                .zip(loaded.palettes.get(usize::from(k.palette)))
                .and_then(|(image, palette)| {
                    load::decode_lightmap(image, palette, files, false).ok()
                });
            if let Some(image) = image {
                pages.insert(k, image);
            }
        }
        let mut out = Self::new(loaded.bsp, loaded.world, base, pages);
        out.stats.failed_sections = loaded.failed_sections;
        out.lightmap_error = loaded.lightmap_error;
        Ok(out)
    }

    /// A world and its images: the draws with no base map, or lit with no
    /// page, are left out.
    pub fn new(
        bsp: TagId,
        world: World,
        base: HashMap<u16, Image>,
        pages: HashMap<LightmapKey, Image>,
    ) -> Self {
        let all = world.draws.len();
        let world = keep_draws(world, &|d| {
            base.contains_key(&d.material) && d.lightmap.is_none_or(|k| pages.contains_key(&k))
        });
        let stats = NativeStats {
            draws: world.draws.len(),
            batches: world.batches.len(),
            materials: world.materials().len(),
            lightmap_pages: world.lightmap_keys().len(),
            untextured: all - world.draws.len(),
            failed_sections: 0,
        };
        Self {
            bsp,
            world,
            base,
            pages,
            stats,
            lightmap_error: None,
        }
    }

    pub fn stats(&self) -> NativeStats {
        self.stats
    }

    /// The world as drawn: what picking and the stats read.
    pub fn world(&self) -> &World {
        &self.world
    }
}

/// What [`NativeWorld::render`] clears before it draws: the colour to a
/// colour, the depth to the far plane. The first world of a frame clears
/// (the colour only when no sky is behind it); later ones load.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Clear {
    pub color: Option<[f64; 3]>,
    pub depth: bool,
}

impl Clear {
    pub const NONE: Self = Self {
        color: None,
        depth: false,
    };
    pub const fn all(color: [f64; 3]) -> Self {
        Self {
            color: Some(color),
            depth: true,
        }
    }
}

/// Every BSP of the scenario ([`NativeLoad::read`]), with why each one
/// that did not load did not, by BSP index.
pub fn read_all<R: Read + Seek>(
    map: &mut Map<R>,
    files: &mut ResourceFiles,
) -> (Vec<NativeLoad>, Vec<(usize, String)>) {
    let count = load::scenario_bsps(map).map_or(0, |b| b.len());
    let (mut loads, mut errors) = (Vec::new(), Vec::new());
    for index in 0..count {
        match NativeLoad::read(map, files, index) {
            Ok(l) => loads.push(l),
            Err(e) => errors.push((index, e)),
        }
    }
    (loads, errors)
}

/// A [`NativeLoad`] on the GPU in one Lighting mode.
pub struct NativeWorld {
    gpu: GpuWorld,
    lighting: Lighting,
    pool: gfx::ring::UploadPool,
}

impl NativeWorld {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        load: &NativeLoad,
        lighting: Lighting,
    ) -> Self {
        let gpu = GpuWorld::new(
            device,
            queue,
            load.world.clone(),
            &mut |m| load.base.get(&m).map(|i| lighting.base(i)),
            &mut |k| load.pages.get(&k).and_then(|i| lighting.page(i)),
        );
        Self {
            gpu,
            lighting,
            pool: Default::default(),
        }
    }

    pub fn lighting(&self) -> Lighting {
        self.lighting
    }

    /// Clear what `clear` names (the colour to its colour, the depth to the
    /// far plane), then draw the world seen by `camera` into the whole
    /// `size` target. Returns what the frustum kept.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        color: &wgpu::TextureView,
        color_format: wgpu::TextureFormat,
        depth: &wgpu::TextureView,
        depth_format: wgpu::TextureFormat,
        size: [u32; 2],
        camera: &Camera,
        clear: Clear,
    ) -> CullStats {
        let view = render_camera(camera, size);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("editor native world"),
        });
        if clear.color.is_some() || clear.depth {
            drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("editor native world clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: match clear.color {
                            Some([r, g, b]) => wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a: 1.0 }),
                            None => wgpu::LoadOp::Load,
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth,
                    depth_ops: Some(wgpu::Operations {
                        load: if clear.depth {
                            wgpu::LoadOp::Clear(1.0)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            }));
        }
        let arena = self.pool.begin();
        let stats = {
            let mut uploads = gfx::ring::EncoderUploads {
                device,
                queue,
                encoder: &mut encoder,
                arena: &arena,
            };
            self.gpu.prepare(&mut uploads, &view, DEPTH_RULE)
        };
        self.gpu.encode(
            device,
            &mut encoder,
            color,
            color_format,
            depth,
            depth_format,
            PassViewport {
                x: 0.0,
                y: 0.0,
                width: size[0] as f32,
                height: size[1] as f32,
            },
            (size[0], size[1]),
        );
        let patches = arena.upload_commands(queue);
        queue.submit(patches.into_iter().chain([encoder.finish()]));
        queue.on_submitted_work_done(move || drop(arena));
        stats
    }
}

#[cfg(test)]
mod tests;
