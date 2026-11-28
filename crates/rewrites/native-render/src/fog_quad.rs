//! The planar fog's screen quad (`0x00025d50`) drawn by the native
//! renderer (docs/graphics/retire-d3d-fog-quad.md). The numbers are
//! `engine::planar_fog`'s; this draws the fan as one quad through
//! [`crate::glow::QuadDrawer`] with the guest's own combiner block.
//!
//! | Guest (`0x00026520`, `0x00034420`) | Here |
//! |---|---|
//! | the block at `0x00484f68`: the final combiner's constant 0 as colour and alpha | [`FogQuadDraw::combiner`], translated as the post's blocks are |
//! | `ZEnable(2)`, `GL_LEQUAL`, no depth write | the quad's stored depth ([`crate::flare_gpu::reference`] of its z and w under the device's W clip) tested `LessEqual` against the bound depth, kept |
//! | blend on, `GL_CONSTANT_COLOR`, `GL_ONE_MINUS_SRC_ALPHA`, `GL_FUNC_ADD`, constant `0x00ffffff` | [`blend`] with [`blend_constant`]: the colour added over the target times one less the fog's alpha, the target's alpha times the same |
//! | colour mask `0x01010101`, cull none, stencil and alpha test off, z bias 0 | every channel written, no cull, no stencil or alpha test |
//! | four vertices, corners `(0, 0)`, `(0, 1)`, `(1, 1)`, `(1, 0)` of the rectangle | one quad over [`FogQuadDraw::rect`] |
//!
//! Read, not run: the vertex program `0x0001c590` binds
//! (`[[[0x00485a80] + 0x5c] + 0x64]`) is taken to pass the position in
//! pixels through, as the glow's screen quads' does; the fill mode is the
//! device's, taken to be solid.

/// Words of a combiner block (`D3DPIXELSHADERDEF`).
pub const WORDS: usize = 60;

/// One fan to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct FogQuadDraw {
    /// The block the setup applies.
    pub combiner: [u32; WORDS],
    /// `(x0, y0, x1, y1)` in the target's guest pixels, `x0 <= x1`,
    /// `y0 <= y1`.
    pub rect: [f32; 4],
    /// Every vertex's z and w.
    pub z: f32,
    pub w: f32,
}

/// Why a fan is not one quad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// Its corners do not form an axis-aligned rectangle at one depth.
    NotRectangle,
    /// A corner is not finite.
    NotFinite,
}

impl Shape {
    pub fn cause(self) -> &'static str {
        match self {
            Shape::NotRectangle => "fan_shape",
            Shape::NotFinite => "fan_unfinite",
        }
    }
}

impl FogQuadDraw {
    /// The quad of a fan whose four positions (x, y, z, w) are in the
    /// order the fan pushes its corners: `(0, 0)`, `(0, 1)`, `(1, 1)`,
    /// `(1, 0)`.
    pub fn from_fan(combiner: [u32; WORDS], p: [[f32; 4]; 4]) -> Result<Self, Shape> {
        if p.iter().flatten().any(|v| !v.is_finite()) {
            return Err(Shape::NotFinite);
        }
        let same = |a: f32, b: f32| a.to_bits() == b.to_bits();
        let rectangle = same(p[0][0], p[1][0])
            && same(p[2][0], p[3][0])
            && same(p[0][1], p[3][1])
            && same(p[1][1], p[2][1])
            && p.iter().all(|v| same(v[2], p[0][2]) && same(v[3], p[0][3]));
        if !rectangle {
            return Err(Shape::NotRectangle);
        }
        let (xa, xb, ya, yb) = (p[0][0], p[2][0], p[0][1], p[1][1]);
        Ok(Self {
            combiner,
            rect: [xa.min(xb), ya.min(yb), xa.max(xb), ya.max(yb)],
            z: p[0][2],
            w: p[0][3],
        })
    }

    /// Whether the quad covers no pixel.
    pub fn empty(&self) -> bool {
        let [x0, y0, x1, y1] = self.rect;
        x1 <= x0 || y1 <= y0
    }
}

/// The setup's blend constant (`0x0004034c`), ARGB.
pub const BLEND_COLOUR: u32 = 0x00ff_ffff;
/// The setup's blend factors and equation (`0x00040344`, `0x00040348`,
/// `0x00040350`): `GL_CONSTANT_COLOR`, `GL_ONE_MINUS_SRC_ALPHA`,
/// `GL_FUNC_ADD`.
pub const BLEND_SOURCE: u32 = 0x8001;
pub const BLEND_DEST: u32 = 0x0303;
pub const BLEND_EQUATION: u32 = 0x8006;
/// `ZEnable(2)` and `GL_LEQUAL`: W-buffered, tested less or equal.
pub const Z_ENABLE: u32 = 2;
pub const Z_FUNC: u32 = 0x0203;

/// The blend constant as red, green, blue and alpha in `[0, 1]`.
pub fn blend_constant(argb: u32) -> [f64; 4] {
    let c = |shift: u32| f64::from((argb >> shift) & 0xff) / 255.0;
    [c(16), c(8), c(0), c(24)]
}

#[cfg(feature = "gpu")]
pub use gpu::{FogQuadGpu, FogQuadTarget, blend};

#[cfg(feature = "gpu")]
mod gpu {
    use super::{BLEND_COLOUR, FogQuadDraw, blend_constant};
    use crate::flare_gpu::{self, DepthTest};
    use crate::glow::{PostSurface, Quad, QuadDepth, QuadDrawer};
    use crate::screen_gpu::{FragmentRaster, PostRaster};

    /// The fan's blend: the constant's factor on colour and alpha (the
    /// NV2A blends alpha with the colour's factors), one less the source's
    /// alpha on the target, added.
    pub fn blend() -> wgpu::BlendState {
        let c = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Constant,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        wgpu::BlendState { color: c, alpha: c }
    }

    /// Where a fan draws: the view's colour (`guest` pixels) and depth,
    /// the submission's arena its uniforms go in.
    pub struct FogQuadTarget<'a> {
        pub device: &'a wgpu::Device,
        pub queue: &'a wgpu::Queue,
        pub uploads: &'a gfx::ring::SubmissionUploads,
        pub color: &'a wgpu::TextureView,
        pub color_format: wgpu::TextureFormat,
        pub depth: &'a wgpu::TextureView,
        pub guest: (u32, u32),
        /// The depth format's full scale as the W-buffer uses it.
        pub z_scale: f32,
    }

    /// The fan's programs and pipelines on one device, kept across frames.
    #[derive(Default)]
    pub struct FogQuadGpu {
        quads: QuadDrawer,
    }

    impl FogQuadGpu {
        pub fn new() -> Self {
            Self::default()
        }

        /// Records `draw` into `encoder` over `t`, tested under `test`.
        /// `Ok(false)`: nothing to draw (an empty rectangle, or a fan the
        /// W clip culls); `Err` names why nothing was recorded.
        pub fn record(
            &mut self,
            t: FogQuadTarget<'_>,
            encoder: &mut wgpu::CommandEncoder,
            draw: &FogQuadDraw,
            test: DepthTest,
        ) -> Result<bool, &'static str> {
            let (ct, dt) = (t.color.texture(), t.depth.texture());
            if !dt.format().has_depth_aspect() || dt.size() != ct.size() || dt.sample_count() != 1 {
                return Err("depth");
            }
            if t.guest.0 == 0 || t.guest.1 == 0 {
                return Err("outside");
            }
            if draw.empty() {
                return Ok(false);
            }
            let probe = scene_extract::flares::Quad { x0: 0, y0: 0, x1: 1, y1: 1, z: draw.z, w: draw.w, area: 1 };
            let Some(reference) = flare_gpu::reference(&probe, test, t.z_scale) else {
                return Ok(false);
            };
            let program = self
                .quads
                .cached(&draw.combiner, [None; 4], false, FragmentRaster::default())
                .map_err(|_| "combiner")?;
            let quad = Quad {
                corners: draw.rect,
                dest: [2.0 / t.guest.0 as f32, 2.0 / t.guest.1 as f32, reference, 0.0],
                taps: [[0.0; 4]; 4],
            };
            let [r, g, b, a] = blend_constant(BLEND_COLOUR);
            let raster = PostRaster {
                blend: Some(blend()),
                blend_constant: wgpu::Color { r, g, b, a },
                writes: wgpu::ColorWrites::ALL,
                ..PostRaster::default()
            };
            let command = self
                .quads
                .prepare_tested(
                    (t.device, t.queue, t.uploads),
                    &program,
                    bytemuck::bytes_of(&quad),
                    None,
                    (PostSurface { view: t.color, guest_size: t.guest }, t.color_format, raster),
                    Some(QuadDepth { view: t.depth, compare: wgpu::CompareFunction::LessEqual }),
                    [None; 4],
                )?
                .ok_or("empty")?;
            command.record(encoder);
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fan(x: [f32; 2], y: [f32; 2]) -> [[f32; 4]; 4] {
        let at = |a: f32, b: f32| [a, b, 0.5, 16_777_215.0];
        [at(x[0], y[0]), at(x[0], y[1]), at(x[1], y[1]), at(x[1], y[0])]
    }

    #[test]
    fn a_fan_of_the_four_corners_is_one_quad() {
        let q = FogQuadDraw::from_fan([0; WORDS], fan([0.0, 640.0], [0.0, 480.0])).unwrap();
        assert_eq!(q.rect, [0.0, 0.0, 640.0, 480.0]);
        assert_eq!((q.z, q.w), (0.5, 16_777_215.0));
        assert!(!q.empty());
        // Reversed corners cover the same pixels (no cull).
        let r = FogQuadDraw::from_fan([0; WORDS], fan([640.0, 0.0], [480.0, 0.0])).unwrap();
        assert_eq!(r.rect, q.rect);
        assert!(FogQuadDraw::from_fan([0; WORDS], fan([5.0, 5.0], [0.0, 1.0])).unwrap().empty());
    }

    #[test]
    fn a_fan_that_is_not_a_rectangle_at_one_depth_is_refused() {
        let mut p = fan([0.0, 640.0], [0.0, 480.0]);
        p[2][0] = 600.0;
        assert_eq!(FogQuadDraw::from_fan([0; WORDS], p), Err(Shape::NotRectangle));
        let mut p = fan([0.0, 640.0], [0.0, 480.0]);
        p[3][3] = 1.0;
        assert_eq!(FogQuadDraw::from_fan([0; WORDS], p), Err(Shape::NotRectangle));
        let mut p = fan([0.0, 640.0], [0.0, 480.0]);
        p[1][1] = f32::INFINITY;
        assert_eq!(FogQuadDraw::from_fan([0; WORDS], p), Err(Shape::NotFinite));
    }

    #[test]
    fn the_blend_constant_is_white_with_alpha_zero() {
        assert_eq!(blend_constant(BLEND_COLOUR), [1.0, 1.0, 1.0, 0.0]);
        assert_eq!(blend_constant(0x80ff_0000), [1.0, 0.0, 0.0, 128.0 / 255.0]);
    }
}
