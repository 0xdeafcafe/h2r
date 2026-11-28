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

/// The other setup's fan (`0x00025e50`, view modes 1, 4 and 5) as the
/// native pass draws it: one strip with its four texture coordinates.
///
/// | Guest (`0x00025e50`, `0x00026710`) | Here |
/// |---|---|
/// | target 3 at stage 0: the depth buffer's `D24S8` words as `LIN_A8R8G8B8`, clamp, point | the bound depth written as those words into an `Rgba8Unorm` copy ([`FogQuadGpu::record_other`]), sampled at texel centres; its guest size the stage's scale (coordinate 0 is in its pixels) |
/// | stages 1 and 2 `DOTPRODUCT` and `DOT_ST` of the `HILO_1` halves with coordinates 1 and 2 | the block translated as the post's are (`psh`'s texture shader) |
/// | the rasterizer's bitmap `[[0x00485a80] + 0x14]` at stage 2, clamp, linear | the decoded bitmap ([`ramp_texture`]'s header), stage scale 1 (the dots are its normalised coordinates) |
/// | W-buffered `GL_GREATER`, no depth write, or no test with `[0x0048575d]` set | [`FogOtherDraw::depth_test`]: the strip's stored depth tested `Greater` against the bound depth |
/// | the planar fog's blend, constant `0x00ffffff` | [`blend`] with [`blend_constant`] |
/// | the weather plate's block when `0x0003a5c0` returns true: stage 0 alone, W-buffered `GL_LEQUAL` unless its depth was held | [`FogOtherDraw::plate`]: no ramp, the strip tested `LessEqual` |
#[derive(Clone, Debug, PartialEq)]
pub struct FogOtherDraw {
    pub combiner: [u32; WORDS],
    /// The strip: each vertex's position x and y (the target's guest
    /// pixels) and its four texture coordinates.
    pub strip: [([f32; 2], [[f32; 4]; 4]); 4],
    /// Every vertex's z and w.
    pub z: f32,
    pub w: f32,
    pub depth_test: bool,
    /// The weather plate's block (`engine::planar_fog::Other::Plate`):
    /// stage 0 alone, tested less or equal.
    pub plate: bool,
    /// Stage 2's ramp header ([`ramp_texture`]); 0 for the plate.
    pub ramp: u32,
}

impl FogOtherDraw {
    /// The strip of a fan whose positions and coordinates are in the
    /// order the fan pushes its corners, `(0, 0)`, `(0, 1)`, `(1, 1)`,
    /// `(1, 0)`: a strip takes the last two swapped.
    pub fn from_fan(combiner: [u32; WORDS], p: [[f32; 4]; 4], t: [[[f32; 4]; 4]; 4], depth_test: bool) -> Result<Self, Shape> {
        if p.iter().flatten().chain(t.iter().flatten().flatten()).any(|v| !v.is_finite()) {
            return Err(Shape::NotFinite);
        }
        let same = |a: f32, b: f32| a.to_bits() == b.to_bits();
        if !p.iter().all(|v| same(v[2], p[0][2]) && same(v[3], p[0][3])) {
            return Err(Shape::NotRectangle);
        }
        let at = |n: usize| ([p[n][0], p[n][1]], t[n]);
        Ok(Self { combiner, strip: [at(0), at(1), at(3), at(2)], z: p[0][2], w: p[0][3], depth_test, plate: false, ramp: 0 })
    }
}

/// The texture header `0x000143c0` binds at stage 2 for the other setup:
/// image 0 of the rasterizer's bitmap `[[0x00485a80] + 0x14]`, or of its
/// default bitmap `+0x64` when that is -1 or has no image 0; its header
/// (`+0x50`) when `0x00012310`'s fast arm takes it: a header with a stamp
/// `+0x70` past the cache's, or one not streamed (`+0x0f` bit 1 clear,
/// `0x0012cd2c`). Otherwise [`Ramp::Streaming`]: the guest's `0x00012310`
/// asks the texture cache, which may start the load and returns the header
/// it has.
///
/// `Err` names what the guest would do that this does not: `ramp_unbound`
/// (nothing bound, the stage keeps the last texture), `ramp_image`,
/// `ramp_unread`.
pub fn ramp_texture(word: &dyn Fn(u32) -> Option<u32>) -> Result<Ramp, &'static str> {
    use crate::decal_material::{RASTERIZER, image_at, tag_data};
    const UNREAD: &str = "ramp_unread";
    let rasterizer = word(RASTERIZER).ok_or(UNREAD)?;
    let default = || word(rasterizer.wrapping_add(0x64)).ok_or(UNREAD);
    let mut bitmap = word(rasterizer.wrapping_add(0x14)).ok_or(UNREAD)?;
    if bitmap == u32::MAX {
        bitmap = default()?;
        if bitmap == u32::MAX {
            return Err("ramp_unbound");
        }
    }
    let mut tag = tag_data(word, bitmap).ok_or(UNREAD)?;
    if tag != 0 && word(tag.wrapping_add(0x44)).ok_or(UNREAD)? as i32 <= 0 {
        bitmap = default()?;
        tag = tag_data(word, bitmap).ok_or(UNREAD)?;
    }
    if tag == 0 || word(tag.wrapping_add(0x44)).ok_or(UNREAD)? as i32 <= 0 {
        return Err("ramp_unbound");
    }
    let image = image_at(word, bitmap, 0).ok_or(UNREAD)?;
    if image == 0 {
        return Err("ramp_image");
    }
    let stamp = word(image.wrapping_add(0x70)).ok_or(UNREAD)? as i32;
    let header = word(image.wrapping_add(0x50)).ok_or(UNREAD)?;
    let streamed = (word(image.wrapping_add(0xc)).ok_or(UNREAD)? >> 24) & 2 != 0;
    let fresh = stamp > word(crate::effect_material::RESIDENT_STAMP).ok_or(UNREAD)? as i32;
    if header != 0 && (fresh || !streamed) {
        return Ok(Ramp::Header(header));
    }
    Ok(Ramp::Streaming { image })
}

/// What [`ramp_texture`] finds for stage 2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ramp {
    /// The resident header.
    Header(u32),
    /// Not resident: the image the guest's `0x00012310` is asked about.
    Streaming { image: u32 },
}

#[cfg(feature = "gpu")]
pub use gpu::{FogOtherTarget, FogQuadGpu, FogQuadTarget, blend};

#[cfg(feature = "gpu")]
mod gpu {
    use super::{BLEND_COLOUR, FogOtherDraw, FogQuadDraw, blend_constant};
    use crate::flare_gpu::{self, DepthTest};
    use crate::glow::{GridFrame, PostSurface, Quad, QuadDepth, QuadDrawer};
    use crate::screen_gpu::{FragmentRaster, PostRaster, StripVertex};

    /// The bound depth as the `D24S8` words the guest's target 3 reads,
    /// one `Rgba8Unorm` texel each in `A8R8G8B8`'s channels (`a` bits
    /// 31..24, `r` 23..16, `g` 15..8, `b` 7..0), the depth rounded as
    /// `gfx::guest_words`' `depth_word` encodes it; stencil 0 (the planes
    /// weight the low half by 0).
    const DEPTH_WORDS: &str = r#"
@group(0) @binding(0) var depth: texture_depth_2d;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4f(p * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) at: vec4f) -> @location(0) vec4f {
    let d = textureLoad(depth, vec2i(at.xy), 0);
    let x = clamp(d, 0.0, 1.0) * 16777215.0;
    let t = trunc(x);
    var z = u32(t);
    if (x - t >= 0.5) { z = z + 1u; }
    let w = z << 8u;
    return vec4f(vec4u((w >> 16u) & 255u, (w >> 8u) & 255u, w & 255u, w >> 24u)) / 255.0;
}
"#;

    /// The depth-words pass and its last copy.
    #[derive(Default)]
    struct DepthWords {
        pipeline: Option<wgpu::RenderPipeline>,
        copy: Option<(wgpu::Texture, wgpu::TextureView)>,
    }

    impl DepthWords {
        /// Records `depth` written as words into the copy, sized as it.
        fn record(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, depth: &wgpu::TextureView) -> Result<wgpu::TextureView, &'static str> {
            let texture = depth.texture();
            if !texture.format().has_depth_aspect() || texture.sample_count() != 1
                || !texture.usage().contains(wgpu::TextureUsages::TEXTURE_BINDING)
            {
                return Err("other_depth_binding");
            }
            let size = texture.size();
            let pipeline = self.pipeline.get_or_insert_with(|| {
                let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("fog other depth words"),
                    source: wgpu::ShaderSource::Wgsl(DEPTH_WORDS.into()),
                });
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("fog other depth words"),
                    layout: None,
                    vertex: wgpu::VertexState {
                        module: &module,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: Default::default(),
                    depth_stencil: None,
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some("fs_main"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())],
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            });
            if self.copy.as_ref().is_none_or(|(t, _)| t.size() != wgpu::Extent3d { depth_or_array_layers: 1, ..size }) {
                let t = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("fog other depth words"),
                    size: wgpu::Extent3d { width: size.width, height: size.height, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                });
                let v = t.create_view(&Default::default());
                self.copy = Some((t, v));
            }
            let Some((_, out)) = self.copy.as_ref() else { return Err("other_depth_binding") };
            let source = texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("fog other depth"),
                aspect: wgpu::TextureAspect::DepthOnly,
                ..Default::default()
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fog other depth words"),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&source) }],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fog other depth words"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: out,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
            drop(pass);
            Ok(out.clone())
        }
    }

    /// Where the other setup's fan draws: the bound colour and depth, and
    /// the decoded ramp bitmap.
    pub struct FogOtherTarget<'a> {
        pub device: &'a wgpu::Device,
        pub queue: &'a wgpu::Queue,
        pub uploads: &'a gfx::ring::SubmissionUploads,
        pub color: &'a wgpu::TextureView,
        pub color_format: wgpu::TextureFormat,
        pub depth: &'a wgpu::TextureView,
        pub guest: (u32, u32),
        pub z_scale: f32,
        /// Stage 2's ramp; `None` for the plate, which samples stage 0
        /// alone.
        pub ramp: Option<&'a wgpu::TextureView>,
    }

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

    /// The fans' programs and pipelines on one device, kept across frames.
    #[derive(Default)]
    pub struct FogQuadGpu {
        quads: QuadDrawer,
        words: DepthWords,
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

        /// Records the other setup's `draw` into `encoder` over `t`,
        /// tested under `test` when it tests depth: first the bound depth
        /// as target 3's words, then the strip sampling them and the ramp.
        /// `Ok(false)`: a fan the W clip culls.
        pub fn record_other(
            &mut self,
            t: FogOtherTarget<'_>,
            encoder: &mut wgpu::CommandEncoder,
            draw: &FogOtherDraw,
            test: Option<DepthTest>,
        ) -> Result<bool, &'static str> {
            let (ct, dt) = (t.color.texture(), t.depth.texture());
            if !dt.format().has_depth_aspect() || dt.size() != ct.size() || dt.sample_count() != 1 {
                return Err("depth");
            }
            if t.guest.0 == 0 || t.guest.1 == 0 {
                return Err("outside");
            }
            let reference = match (draw.depth_test, test) {
                (false, _) => None,
                (true, None) => return Err("depth_clip"),
                (true, Some(test)) => {
                    let probe = scene_extract::flares::Quad { x0: 0, y0: 0, x1: 1, y1: 1, z: draw.z, w: draw.w, area: 1 };
                    let Some(r) = flare_gpu::reference(&probe, test, t.z_scale) else {
                        return Ok(false);
                    };
                    Some(r)
                }
            };
            if !draw.plate && t.ramp.is_none() {
                return Err("ramp");
            }
            let words = self.words.record(t.device, encoder, t.depth)?;
            // Stage 0 in the target's guest pixels, stage 2 in the ramp's
            // normalised coordinates; the plate samples stage 0 alone.
            let ramp_scale = (!draw.plate).then_some((1, 1));
            let program = self
                .quads
                .grid_program(&draw.combiner, [Some(t.guest), None, ramp_scale, None])
                .map_err(|_| "other_combiner")?;
            let compare = if draw.plate { wgpu::CompareFunction::LessEqual } else { wgpu::CompareFunction::Greater };
            let strip: Vec<StripVertex> = draw
                .strip
                .iter()
                .map(|&([x, y], texcoords)| StripVertex { position: [x, y, 0.0, 1.0], texcoords })
                .collect();
            let [r, g, b, a] = blend_constant(BLEND_COLOUR);
            let raster = PostRaster {
                blend: Some(blend()),
                blend_constant: wgpu::Color { r, g, b, a },
                writes: wgpu::ColorWrites::ALL,
                ..PostRaster::default()
            };
            let frame = GridFrame { dest: [2.0 / t.guest.0 as f32, 2.0 / t.guest.1 as f32, reference.unwrap_or(0.0), 0.0] };
            let command = self
                .quads
                .prepare_tested(
                    (t.device, t.queue, t.uploads),
                    &program,
                    bytemuck::bytes_of(&frame),
                    Some(std::slice::from_ref(&strip)),
                    (PostSurface { view: t.color, guest_size: t.guest }, t.color_format, raster),
                    reference.map(|_| QuadDepth { view: t.depth, compare }),
                    [Some(&words), None, t.ramp.filter(|_| !draw.plate), None],
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

    #[test]
    fn the_other_fan_is_a_strip_with_its_coordinates() {
        let p = [[0.0, 0.0, 0.5, 9.0], [0.0, 480.0, 0.5, 9.0], [640.0, 480.0, 0.5, 9.0], [640.0, 0.0, 0.5, 9.0]];
        let t = [0.0f32, 1.0, 2.0, 3.0].map(|n| [[n; 4]; 4]);
        let d = FogOtherDraw::from_fan([0; WORDS], p, t, true).unwrap();
        let xy: Vec<[f32; 2]> = d.strip.iter().map(|v| v.0).collect();
        assert_eq!(xy, [[0.0, 0.0], [0.0, 480.0], [640.0, 0.0], [640.0, 480.0]]);
        assert_eq!(d.strip[2].1, [[3.0; 4]; 4]);
        assert_eq!((d.z, d.w), (0.5, 9.0));
        let mut bad = t;
        bad[1][2][0] = f32::NAN;
        assert_eq!(FogOtherDraw::from_fan([0; WORDS], p, bad, true), Err(Shape::NotFinite));
    }

    #[test]
    fn the_ramp_is_image_0_of_the_rasterizers_bitmap_or_its_default() {
        use crate::decal_material::{RASTERIZER, TAG_TABLE};
        use std::collections::HashMap;
        let (raster, table, tag, images, image) = (0x1000, 0x2000, 0x3000, 0x4000, 0x4000);
        let mut m: HashMap<u32, u32> = HashMap::new();
        m.insert(RASTERIZER, raster);
        m.insert(raster + 0x14, 0xe000_0002);
        m.insert(raster + 0x64, u32::MAX);
        m.insert(TAG_TABLE, table);
        m.insert(table + 2 * 16 + 8, tag);
        m.insert(tag + 0x44, 1);
        m.insert(tag + 0x48, images);
        m.insert(image + 0x70, 5);
        m.insert(image + 0x50, 0x00ab_cd00);
        m.insert(crate::effect_material::RESIDENT_STAMP, 4);
        let word = |a: u32| m.get(&a).copied();
        assert_eq!(ramp_texture(&word), Ok(Ramp::Header(0x00ab_cd00)));
        // An old stamp: a header not streamed is still taken (0x0012cd2c),
        // a streamed one (`+0x0f` bit 1) or none goes to the cache.
        let mut stale = m.clone();
        stale.insert(crate::effect_material::RESIDENT_STAMP, 5);
        assert_eq!(ramp_texture(&|a| stale.get(&a).copied()), Ok(Ramp::Header(0x00ab_cd00)));
        stale.insert(image + 0xc, 0x0200_0000);
        assert_eq!(ramp_texture(&|a| stale.get(&a).copied()), Ok(Ramp::Streaming { image }));
        let mut empty = m.clone();
        empty.insert(image + 0x50, 0);
        assert_eq!(ramp_texture(&|a| empty.get(&a).copied()), Ok(Ramp::Streaming { image }));
        let mut none = m.clone();
        none.insert(raster + 0x14, u32::MAX);
        assert_eq!(ramp_texture(&|a| none.get(&a).copied()), Err("ramp_unbound"));
    }

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
