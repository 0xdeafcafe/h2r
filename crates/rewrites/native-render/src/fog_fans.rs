//! The second fog's composite fan (`0x00025dc0` with `dl` alone, the
//! setup `0x000276d0`) drawn by the native renderer
//! (docs/graphics/retire-d3d-fog-fans.md). The numbers are
//! `engine::fog_fans::composite`'s; this draws the fan as one strip
//! through [`crate::glow::QuadDrawer::grid_raster`] with the guest's own
//! combiner block, into the view's target and sampling target 3, both
//! the frame graph's surfaces by role (the device's slots).
//!
//! | Guest (`0x000276d0`, `0x00027960`) | Here |
//! |---|---|
//! | the block at `0x00484f68`: stage 0's texture times the colour | [`CompositeDraw::combiner`], translated as the post's blocks are, stage 0 bound |
//! | target 3 at stage 0, clamp, point | the slot's sample view at stage 0, its guest size the stage's scale (attribute 1 is in its pixels, so each fragment samples its own texel's centre) |
//! | blend on, `GL_CONSTANT_COLOR`, `GL_ONE_MINUS_SRC_ALPHA`, add, constant `0x00ffffff` | [`crate::fog_quad::blend`] with the same constant |
//! | colour mask `0x01010101`, alpha test, cull, stencil and depth test off | every channel written, no test, no cull |
//! | four vertices, corners `(0, 0)`, `(0, 1)`, `(1, 1)`, `(1, 0)` | one four-vertex strip, `(0, 0)`, `(0, 1)`, `(1, 0)`, `(1, 1)` |
//!
//! Read, not run: the vertex program `0x0001c590` is taken to pass the
//! position in pixels through, as the glow's screen quads' does; with the
//! depth test off the position's z and w change nothing.

use crate::fog_quad::WORDS;

/// The composite's fan, as the native pass draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct CompositeDraw {
    pub combiner: [u32; WORDS],
    /// The strip: each vertex's position x and y (the target's guest
    /// pixels) and attribute 1 (stage 0's coordinate, target 3's pixels).
    pub strip: [([f32; 2], [f32; 4]); 4],
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
            Refusal::NotFinite => "composite_unfinite",
        }
    }
}

impl CompositeDraw {
    /// The strip of a fan whose positions and attribute 1 are in the order
    /// the fan pushes its corners, `(0, 0)`, `(0, 1)`, `(1, 1)`, `(1, 0)`:
    /// a strip takes the last two swapped.
    pub fn from_fan(combiner: [u32; WORDS], positions: [[f32; 4]; 4], coordinates: [[f32; 4]; 4]) -> Result<Self, Refusal> {
        if positions.iter().chain(&coordinates).flatten().any(|v| !v.is_finite()) {
            return Err(Refusal::NotFinite);
        }
        let at = |n: usize| ([positions[n][0], positions[n][1]], coordinates[n]);
        Ok(Self { combiner, strip: [at(0), at(1), at(3), at(2)] })
    }
}

#[cfg(feature = "gpu")]
pub use gpu::{FogFansGpu, FogFansTarget};

#[cfg(feature = "gpu")]
mod gpu {
    use super::CompositeDraw;
    use crate::fog_quad::{BLEND_COLOUR, blend, blend_constant};
    use crate::glow::{GridFrame, PostSurface, QuadDrawer};
    use crate::screen_gpu::{PostRaster, StripVertex};

    /// Where the composite draws: the view's target (its attachment and
    /// guest size) and target 3 (its sample view and guest size), the
    /// submission's arena its uniforms go in.
    pub struct FogFansTarget<'a> {
        pub device: &'a wgpu::Device,
        pub queue: &'a wgpu::Queue,
        pub uploads: &'a gfx::ring::SubmissionUploads,
        pub dest: PostSurface<'a>,
        pub dest_format: wgpu::TextureFormat,
        pub source: &'a wgpu::TextureView,
        pub source_guest: (u32, u32),
    }

    /// The fans' programs and pipelines on one device, kept across frames.
    #[derive(Default)]
    pub struct FogFansGpu {
        quads: QuadDrawer,
    }

    impl FogFansGpu {
        pub fn new() -> Self {
            Self::default()
        }

        /// Records `draw` into `encoder` over `t`. `Err` names why nothing
        /// was recorded.
        pub fn record(&mut self, t: FogFansTarget<'_>, encoder: &mut wgpu::CommandEncoder, draw: &CompositeDraw) -> Result<(), &'static str> {
            let (w, h) = t.dest.guest_size;
            if w == 0 || h == 0 || t.source_guest.0 == 0 || t.source_guest.1 == 0 {
                return Err("composite_size");
            }
            let program = self
                .quads
                .grid_program(&draw.combiner, [Some(t.source_guest), None, None, None])
                .map_err(|_| "composite_combiner")?;
            let strip: Vec<StripVertex> = draw
                .strip
                .iter()
                .map(|&([x, y], t1)| StripVertex { position: [x, y, 0.0, 1.0], texcoords: [t1, [0.0; 4], [0.0; 4], [0.0; 4]] })
                .collect();
            let [r, g, b, a] = blend_constant(BLEND_COLOUR);
            let raster = PostRaster {
                blend: Some(blend()),
                blend_constant: wgpu::Color { r, g, b, a },
                writes: wgpu::ColorWrites::ALL,
                ..PostRaster::default()
            };
            let command = self
                .quads
                .grid_raster(
                    (t.device, t.queue, t.uploads),
                    &program,
                    &GridFrame::of((w, h)),
                    &[strip],
                    (t.dest, t.dest_format, raster),
                    [Some(t.source), None, None, None],
                )?
                .ok_or("composite_empty")?;
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
        let d = CompositeDraw::from_fan([0; WORDS], corners([0.0, 640.0], [0.0, 480.0], 0.5), corners([0.0, 640.0], [0.0, 480.0], 0.0)).unwrap();
        let xy: Vec<[f32; 2]> = d.strip.iter().map(|v| v.0).collect();
        assert_eq!(xy, [[0.0, 0.0], [0.0, 480.0], [640.0, 0.0], [640.0, 480.0]]);
        assert_eq!(d.strip[2].1, [640.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn an_unfinite_fan_is_refused() {
        let mut p = corners([0.0, 640.0], [0.0, 480.0], 0.5);
        p[1][1] = f32::NAN;
        let t = corners([0.0, 640.0], [0.0, 480.0], 0.0);
        assert_eq!(CompositeDraw::from_fan([0; WORDS], p, t), Err(Refusal::NotFinite));
    }
}
