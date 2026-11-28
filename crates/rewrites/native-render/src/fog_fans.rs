//! The second fog's screen fans (`0x00025dc0`) drawn by the native
//! renderer (docs/graphics/retire-d3d-fog-fans.md). The numbers are
//! `engine::fog_fans`'; this draws each fan as one four-vertex strip
//! through [`crate::glow::QuadDrawer::grid_raster`] with the guest's own
//! combiner block, on the frame graph's surfaces for their roles (the
//! device's slots) and the fog's bitmaps loaded at map load.
//!
//! | Fan (setup) | Into | Stages | Raster |
//! |---|---|---|---|
//! | composite (`0x000276d0`) | the view's target | 0: target 3 | blend [`crate::fog_quad::blend`], constant `0x00ffffff` |
//! | layers (`0x00026880`) | target `0x13` | 0..3: the fog patch's bitmap, each layer's image, wrap | blend off |
//! | layers' composite, weather plate (`0x0003a5c0` true) | as the set branch | 0: target 3; 1: target `0x13` | as the set branch |
//! | layers' composite, set branch (`0x00026f81`) | the view's target, or target 3 | 0: target 3; 1: dot product; 2: the rasterizer bitmap (`DOT_ST`); 3: target `0x13` | into the view as the composite; into target 3 blue alone, blend off |
//!
//! Every fan has alpha test, cull, stencil and depth test off and its
//! corners `(0, 0)`, `(0, 1)`, `(1, 1)`, `(1, 0)`, which a strip takes with
//! the last two swapped. A target stage's coordinates are in its guest
//! pixels ([`Coords::Pixels`]); a bitmap's are normalised. A fan into
//! target 3 that samples target 3 samples a copy of it made just before
//! (the draw writes blue alone, so the copy is what each fragment reads).
//!
//! Read, not run: the vertex program `0x0001c590` is taken to pass the
//! position in pixels through, as the glow's screen quads' does; with the
//! depth test off the position's z and w change nothing. Every stage
//! filters linearly (target 3 at stage 0 is set to point; its
//! coordinates fall on texel centres).

use crate::fog_quad::WORDS;

/// How a stage's coordinates address it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coords {
    /// In the bound target's guest pixels (a linear target).
    Pixels,
    /// Normalised (a bitmap, or a dot stage's result).
    Unit,
}

/// One stage of a fan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FanStage {
    pub coords: Coords,
    /// D3D address 1 (wrap), else clamp.
    pub wrap: bool,
}

/// How a fan writes its target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FanRaster {
    /// Blend on, `GL_CONSTANT_COLOR` over `GL_ONE_MINUS_SRC_ALPHA`, add,
    /// constant `0x00ffffff`, every channel.
    Constant,
    /// Blend off, every channel.
    Opaque,
    /// Blend off, blue alone (colour write `1`).
    BlueOnly,
}

/// One fan, as the native pass draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct FanDraw {
    pub combiner: [u32; WORDS],
    /// The strip: each vertex's position x and y (the target's guest
    /// pixels) and attributes 1..4 (stages 0..3).
    pub strip: [([f32; 2], [[f32; 4]; 4]); 4],
    /// What each stage samples, `None` for one that samples nothing (a
    /// dot product stage).
    pub stages: [Option<FanStage>; 4],
    pub raster: FanRaster,
}

/// Why a fan cannot be drawn as given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A position or coordinate is not finite.
    NotFinite,
}

impl Refusal {
    pub fn cause(self) -> &'static str {
        match self {
            Refusal::NotFinite => "fan_unfinite",
        }
    }
}

impl FanDraw {
    /// The strip of a fan whose positions and attributes are in the order
    /// the fan pushes its corners, `(0, 0)`, `(0, 1)`, `(1, 1)`, `(1, 0)`:
    /// a strip takes the last two swapped. Only the stages `stages`
    /// names need finite coordinates.
    pub fn from_fan(
        combiner: [u32; WORDS],
        positions: [[f32; 4]; 4],
        coordinates: [[[f32; 4]; 4]; 4],
        stages: [Option<FanStage>; 4],
        raster: FanRaster,
    ) -> Result<Self, Refusal> {
        let used = |c: &[[f32; 4]; 4]| c.iter().zip(&stages).filter(|(_, s)| s.is_some()).flat_map(|(v, _)| *v).collect::<Vec<_>>();
        if positions.iter().flat_map(|p| [p[0], p[1]]).chain(coordinates.iter().flat_map(used)).any(|v| !v.is_finite()) {
            return Err(Refusal::NotFinite);
        }
        let at = |n: usize| ([positions[n][0], positions[n][1]], coordinates[n]);
        Ok(Self { combiner, strip: [at(0), at(1), at(3), at(2)], stages, raster })
    }

    /// The composite (`dl` alone): stage 0 target 3, blended.
    pub fn composite(combiner: [u32; WORDS], positions: [[f32; 4]; 4], coordinates: [[f32; 4]; 4]) -> Result<Self, Refusal> {
        let pixels = Some(FanStage { coords: Coords::Pixels, wrap: false });
        let coordinates = coordinates.map(|c| [c, [0.0; 4], [0.0; 4], [0.0; 4]]);
        Self::from_fan(combiner, positions, coordinates, [pixels, None, None, None], FanRaster::Constant)
    }
}

#[cfg(feature = "gpu")]
pub use gpu::{FanTarget, FogFansGpu};

#[cfg(feature = "gpu")]
mod gpu {
    use std::collections::HashMap;

    use super::{Coords, FanDraw, FanRaster};
    use crate::fog_quad::{BLEND_COLOUR, blend, blend_constant};
    use crate::glow::{GridFrame, PostSurface, QuadDrawer};
    use crate::gpu::Image;
    use crate::screen_gpu::{PostRaster, StripVertex};

    /// Where a fan draws, its uniforms' arena, and what each stage
    /// samples with its guest size (a bitmap's is unused).
    pub struct FanTarget<'a> {
        pub device: &'a wgpu::Device,
        pub queue: &'a wgpu::Queue,
        pub uploads: &'a gfx::ring::SubmissionUploads,
        pub dest: PostSurface<'a>,
        pub dest_format: wgpu::TextureFormat,
        pub stages: [Option<(&'a wgpu::TextureView, (u32, u32))>; 4],
    }

    /// The fans' programs, pipelines, bitmaps and target-3 copy on one
    /// device, kept across frames.
    #[derive(Default)]
    pub struct FogFansGpu {
        quads: QuadDrawer,
        /// The fog's bitmaps by (datum, image), for the image set they
        /// were uploaded from.
        bitmaps: HashMap<(u32, u32), wgpu::TextureView>,
        generation: u64,
        snapshot: Option<wgpu::Texture>,
    }

    impl FogFansGpu {
        pub fn new() -> Self {
            Self::default()
        }

        /// `image`'s texture, uploaded once per `generation` of the map's
        /// image set.
        pub fn bitmap(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, generation: u64, key: (u32, u32), image: &Image) -> wgpu::TextureView {
            if generation != self.generation {
                self.bitmaps.clear();
                self.generation = generation;
            }
            self.bitmaps
                .entry(key)
                .or_insert_with(|| crate::gpu::texture(device, queue, image, "native fog fans bitmap"))
                .clone()
        }

        /// A copy of `source`'s texture as it is now, recorded into
        /// `encoder`, for a fan that samples the target it draws into.
        pub fn snapshot(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, source: &wgpu::TextureView) -> Result<wgpu::TextureView, &'static str> {
            let s = source.texture();
            if !s.usage().contains(wgpu::TextureUsages::COPY_SRC) || s.mip_level_count() != 1 || s.sample_count() != 1 {
                return Err("fan_snapshot");
            }
            let fits = self.snapshot.as_ref().is_some_and(|t| t.size() == s.size() && t.format() == s.format());
            if !fits {
                self.snapshot = Some(device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("native fog fans snapshot"),
                    size: s.size(),
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: s.format(),
                    usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                }));
            }
            let Some(copy) = self.snapshot.as_ref() else { return Err("fan_snapshot") };
            encoder.copy_texture_to_texture(s.as_image_copy(), copy.as_image_copy(), s.size());
            Ok(copy.create_view(&wgpu::TextureViewDescriptor::default()))
        }

        /// Records `draw` into `encoder` over `t`. `Err` names why nothing
        /// was recorded.
        pub fn record(&mut self, t: FanTarget<'_>, encoder: &mut wgpu::CommandEncoder, draw: &FanDraw) -> Result<(), &'static str> {
            let (w, h) = t.dest.guest_size;
            if w == 0 || h == 0 {
                return Err("fan_size");
            }
            let mut sizes = [None; 4];
            let mut views = [None; 4];
            for (n, s) in draw.stages.iter().enumerate() {
                let Some(s) = s else { continue };
                let (view, size) = t.stages[n].ok_or("fan_stage")?;
                sizes[n] = Some(match s.coords {
                    Coords::Pixels if size.0 == 0 || size.1 == 0 => return Err("fan_stage_size"),
                    Coords::Pixels => size,
                    Coords::Unit => (1, 1),
                });
                views[n] = Some(view);
            }
            let program = self.quads.grid_program(&draw.combiner, sizes).map_err(|_| "fan_combiner")?;
            let strip: Vec<StripVertex> =
                draw.strip.iter().map(|&([x, y], texcoords)| StripVertex { position: [x, y, 0.0, 1.0], texcoords }).collect();
            let wrap = draw.stages.map(|s| s.is_some_and(|s| s.wrap));
            let raster = match draw.raster {
                FanRaster::Constant => {
                    let [r, g, b, a] = blend_constant(BLEND_COLOUR);
                    PostRaster { blend: Some(blend()), blend_constant: wgpu::Color { r, g, b, a }, wrap, ..PostRaster::default() }
                }
                FanRaster::Opaque => PostRaster { wrap, ..PostRaster::default() },
                FanRaster::BlueOnly => PostRaster { writes: wgpu::ColorWrites::BLUE, wrap, ..PostRaster::default() },
            };
            let command = self
                .quads
                .grid_raster((t.device, t.queue, t.uploads), &program, &GridFrame::of((w, h)), &[strip], (t.dest, t.dest_format, raster), views)?
                .ok_or("fan_empty")?;
            command.record(encoder);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corners(x: [f32; 2], y: [f32; 2], z: f32) -> [[f32; 4]; 4] {
        [[x[0], y[0], z, 1.0], [x[0], y[1], z, 1.0], [x[1], y[1], z, 1.0], [x[1], y[0], z, 1.0]]
    }

    #[test]
    fn the_fan_is_a_strip_with_its_last_two_swapped() {
        let d = FanDraw::composite([0; WORDS], corners([0.0, 640.0], [0.0, 480.0], 0.5), corners([0.0, 640.0], [0.0, 480.0], 0.0)).unwrap();
        let xy: Vec<[f32; 2]> = d.strip.iter().map(|v| v.0).collect();
        assert_eq!(xy, [[0.0, 0.0], [0.0, 480.0], [640.0, 0.0], [640.0, 480.0]]);
        assert_eq!(d.strip[2].1[0], [640.0, 0.0, 0.0, 1.0]);
        assert_eq!(d.raster, FanRaster::Constant);
        assert_eq!(d.stages[0], Some(FanStage { coords: Coords::Pixels, wrap: false }));
        assert_eq!(d.stages[1..], [None; 3]);
    }

    #[test]
    fn an_unfinite_fan_is_refused_only_where_it_is_used() {
        let mut p = corners([0.0, 640.0], [0.0, 480.0], 0.5);
        p[1][1] = f32::NAN;
        let t = corners([0.0, 640.0], [0.0, 480.0], 0.0);
        assert_eq!(FanDraw::composite([0; WORDS], p, t), Err(Refusal::NotFinite));
        // A NaN in an unsampled stage, or in a position's z, draws.
        let p = corners([0.0, 640.0], [0.0, 480.0], f32::NAN);
        let mut c = t.map(|c| [c, [0.0; 4], [0.0; 4], [0.0; 4]]);
        c[0][2][0] = f32::NAN;
        let stage = Some(FanStage { coords: Coords::Unit, wrap: true });
        assert!(FanDraw::from_fan([0; WORDS], p, c, [stage, None, None, None], FanRaster::Opaque).is_ok());
        assert_eq!(FanDraw::from_fan([0; WORDS], p, c, [stage, None, stage, None], FanRaster::Opaque), Err(Refusal::NotFinite));
    }
}
