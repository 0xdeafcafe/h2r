//! The flare glow `0x00046ea0` drawn by the native renderer
//! (docs/graphics/d3d-visibility.md, "The glow in Rust"). The glow's
//! numbers are `scene_extract::flares::glow`'s; this draws its fans with
//! the guest's own combiner blocks, each a quad through
//! [`crate::glow::QuadDrawer`].
//!
//! | Guest | Here |
//! |---|---|
//! | the mask fan (`0x000470b2`..`0x00047306`): alpha only, depth `ALWAYS`, block [`CLEAR`] | the view's alpha over the rectangle set to 0 |
//! | the masked sample (`0x00047331`..`0x00047512`): the bitmap `[[0x00485a80] + 0x34]` image 0 on stage 0, alpha only, `LESSEQUAL` at `z = w = 16777215` under the W clip, block [`MASKED`] | the view's alpha `min(1, 4 a)` where the fan passes the stored depth ([`MASK_DEPTH`], `flare_gpu::reference`) |
//! | `0x00046230` into targets `0x10` and `0x11`: the view on stage 0 over the rectangle, the bitmap on stage 1, block [`DOWN`] | both squares of [`side`] texels: the view's alpha as colour, `min(1, 2 a * 192/255)` of the bitmap as alpha |
//! | `0x000467c0(0x10, 0x11, 4)`: four taps half a texel apart on the diagonals ([`BLUR_TAP`]), block [`blur`], blended `DSTALPHA`, `ZERO`, colour only | four passes ping-ponging the squares, ending in the first |
//! | the composite's sixteen fans (`0x00047740`..`0x0004785d`): the result on stage 0, `v0.a` the ring's weight, block [`COMPOSITE`], `SRCALPHA`, `ONE`, colour only | sixteen quads over the view's colour |
//!
//! The view's alpha over the rectangle is what the masked sample leaves;
//! the composite writes colour only.
//!
//! Read, not run: engine vertex shader 10 (`lens_flare`, the view's fans)
//! is taken to pass the position in pixels through, `v3` (the 0/1
//! shorts) to `oT0` and `v9` (the weight in w) to `oD0`; vertex shader 1
//! (the down-sample and the blur) to apply the two-row texture matrices
//! the calls load at hardware slot `0x12`. The blur's stages address
//! `BORDER` (4) with a border colour the glow does not set; here they
//! clamp to the edge, which differs only in the square's outer texel
//! ring, where `rasterizer\glow`'s alpha (the blend's factor) is at most
//! 3/255.

use scene_extract::flares::{Glow, RINGS, Ring};

/// One glow to draw: the rectangle, the composite's rings and the
/// down-sample's square.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlowDraw {
    pub glow: Glow,
    pub rings: [Ring; RINGS],
    /// The square's corners (`scene_extract::flares::glow_k::SOURCE`), in
    /// guest pixels.
    pub source: [f32; 2],
}

/// The masked sample's bitmap, decoded from the texture the guest binds
/// (`[[0x00485a80] + 0x34]`, image 0): RGBA8, rows tight. `key` names
/// the guest texture (a hash of its header, the header's identity words
/// and its bytes), so a texture made once serves every frame until it
/// changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitmap {
    pub key: u64,
    pub size: (u32, u32),
    pub rgba: Vec<u8>,
}

/// The down-sample's side in host texels: the guest square's, times the
/// render scale, held to `[1, 512]`.
pub fn side(source: [f32; 2], guest: (u32, u32), host: (u32, u32)) -> u32 {
    let scale = if guest.0 == 0 { 1.0 } else { host.0 as f32 / guest.0 as f32 };
    let s = ((source[1] - source[0]).abs().round() * scale).round();
    if s.is_finite() { s.clamp(1.0, 512.0) as u32 } else { 1 }
}

/// The masked sample's `z` and `w` (`0x4b7fffff`).
pub const MASK_DEPTH: f32 = 16_777_215.0;

/// The blur passes `0x000467c0` makes.
pub const BLURS: u32 = 4;

/// The blur's texture-matrix offsets, in the square's normalised
/// coordinates (`[0x0045e488]`, `[0x0045df64]`): stage 0 takes the first
/// on both axes, stage 1 the second, stage 2 `(first, second)`, stage 3
/// `(second, first)`.
pub const BLUR_TAP: [f32; 2] = [-0.0078125, 0.0078125];

/// Words of a combiner block (`D3DPIXELSHADERDEF`).
pub const WORDS: usize = 60;

const fn block(writes: &[(usize, u32)]) -> [u32; WORDS] {
    let mut w = [0; WORDS];
    let mut i = 0;
    while i < writes.len() {
        w[writes[i].0] = writes[i].1;
        i += 1;
    }
    w
}

/// The mask fan's block (`0x00047154`..`0x00047177`): one combiner, the
/// final combiner's alpha `G` the final constant 0's alpha
/// (`PSFinalCombinerConstant0`, written 0): alpha 0.
pub const CLEAR: [u32; WORDS] = block(&[(53, 1), (43, 0), (9, 0x1100)]);

/// The masked sample's block (`0x00047435`..`0x00047475`): `r0.a = t0.a`
/// shifted left 2, the output `r0.a`.
pub const MASKED: [u32; WORDS] = block(&[(54, 1), (53, 1), (0, 0x1820_0000), (26, 0x0002_00c0), (8, 0x1c), (9, 0x1c00)]);

/// The down-sample's block (`0x00046547`..`0x00046597`): `r0.a = t1.a *
/// c0.a` shifted left 1 (`c0` `0xc0000000`), the output `t0.a` as colour
/// and `r0.a` as alpha.
pub const DOWN: [u32; WORDS] = block(&[
    (54, 0x21),
    (53, 1),
    (10, 0xc000_0000),
    (0, 0x1911_0000),
    (26, 0x0001_00c0),
    (8, 0x18),
    (9, 0x1c00),
]);

/// Blur pass `pass`'s block (`0x00046a55`..`0x00046ab9`, `c0` rewritten
/// per pass at `0x00046bd7`): `r0.a = (t0.b + t1.b) / 2`, `r0.rgb = (t2 +
/// t3) / 2`, then `r0.rgb = c0.a (r0.a + r0.rgb)`, `c0.a` 1 on the first
/// pass and `0x7f / 255` after; the output `r0.rgb`.
pub const fn blur(pass: u32) -> [u32; WORDS] {
    let c0 = if pass == 0 { 0xff00_0000 } else { 0x7f00_0000 };
    block(&[
        (26, 0xc00),
        (45, 0xc00),
        (46, 0xc00),
        (54, 0x8421),
        (53, 2),
        (10, c0),
        (0, 0x08a0_09a0),
        (34, 0x0aa0_0ba0),
        (35, 0x1c11_0c11),
        (8, 0x0c),
    ])
}

/// The composite's block (`0x00047682`..`0x000476ea`): `r0.a = 2 t0.b -
/// 1`, `r0.rgb = (1 - r0.a) c0 + r0.a c1` (`c0` `0x00b0b080`, `c1`
/// white), the output `r0.rgb * t0.rgb` with `v0.a` as alpha.
pub const COMPOSITE: [u32; WORDS] = block(&[
    (54, 1),
    (53, 2),
    (10, 0x00b0_b080),
    (18, 0x00ff_ffff),
    (0, 0x4820_0000),
    (26, 0xc0),
    (35, 0x3c01_1c02),
    (46, 0xc00),
    (8, 0x0c08_0000),
    (9, 0x1400),
]);

#[cfg(feature = "gpu")]
pub use gpu::{FlareDrawGpu, FlareTarget};

#[cfg(feature = "gpu")]
mod gpu {
    use super::{BLUR_TAP, BLURS, Bitmap, COMPOSITE, CLEAR, DOWN, GlowDraw, MASKED, WORDS, blur, side};
    use crate::glow::{PostSurface, Quad, QuadCommand, QuadDepth, QuadDrawer};
    use crate::screen_gpu::{FragmentRaster, PostRaster};

    /// The squares' format: targets `0x10` and `0x11` are A8R8G8B8.
    const SQUARE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

    /// Every sampled stage is addressed in normalised coordinates by the
    /// quads' taps, so the programs see stages of size 1.
    const UNIT: Option<(u32, u32)> = Some((1, 1));

    /// A quad over `rect` (guest pixels of a `guest` target) whose stage 0
    /// covers `[0, 1]` across it, at depth `z` with `v0.a` `weight`.
    pub(super) fn over(rect: [f32; 4], guest: (u32, u32), z: f32, weight: f32) -> Quad {
        let [x0, y0, x1, y1] = rect;
        let (w, h) = (x1 - x0, y1 - y0);
        let stage = [-x0, -y0, 1.0 / w, 1.0 / h];
        Quad {
            corners: rect,
            dest: [2.0 / guest.0 as f32, 2.0 / guest.1 as f32, z, weight],
            taps: [stage, [0.0; 4], [0.0; 4], [0.0; 4]],
        }
    }

    /// The down-sample's quad over a square of `side` texels: stage 0
    /// the view (`guest` pixels) over `rect`, stage 1 the bitmap over the
    /// square, as `0x00046230`'s texture matrices place them.
    pub(super) fn down(rect: [f32; 4], guest: (u32, u32), side: u32) -> Quad {
        let [x0, y0, x1, y1] = rect;
        let s = side as f32;
        let (sx, sy) = ((x1 - x0) / s, (y1 - y0) / s);
        Quad {
            corners: [0.0, 0.0, s, s],
            dest: [2.0 / s, 2.0 / s, 0.0, 0.0],
            taps: [
                [x0 / sx, y0 / sy, sx / guest.0 as f32, sy / guest.1 as f32],
                [0.0, 0.0, 1.0 / s, 1.0 / s],
                [0.0; 4],
                [0.0; 4],
            ],
        }
    }

    /// A blur pass's quad over a square of `side` texels: stage `k` the
    /// square offset by [`BLUR_TAP`] as `0x000467c0` loads them.
    pub(super) fn blurred(side: u32) -> Quad {
        let s = side as f32;
        let [a, b] = BLUR_TAP;
        let tap = |u: f32, v: f32| [u * s, v * s, 1.0 / s, 1.0 / s];
        Quad { corners: [0.0, 0.0, s, s], dest: [2.0 / s, 2.0 / s, 0.0, 0.0], taps: [tap(a, a), tap(b, b), tap(a, b), tap(b, a)] }
    }

    /// The same blend on colour and alpha (the NV2A blends alpha with the
    /// colour's factors).
    fn blend(src: wgpu::BlendFactor, dst: wgpu::BlendFactor) -> Option<wgpu::BlendState> {
        let c = wgpu::BlendComponent { src_factor: src, dst_factor: dst, operation: wgpu::BlendOperation::Add };
        Some(wgpu::BlendState { color: c, alpha: c })
    }

    /// The glow's quads, the two squares and the bitmap on one device,
    /// kept across frames.
    #[derive(Default)]
    pub struct FlareDrawGpu {
        quads: QuadDrawer,
        /// Targets `0x10` and `0x11` at their side.
        squares: Option<(u32, [wgpu::TextureView; 2])>,
        bitmap: Option<(u64, wgpu::TextureView)>,
    }

    fn square(device: &wgpu::Device, side: u32) -> wgpu::TextureView {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("native flare glow square"),
                size: wgpu::Extent3d { width: side, height: side, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: SQUARE_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default())
    }

    /// Where a glow draws: the view's colour (`guest` pixels) and depth,
    /// the submission's arena its uniforms go in.
    pub struct FlareTarget<'a> {
        pub device: &'a wgpu::Device,
        pub queue: &'a wgpu::Queue,
        pub uploads: &'a gfx::ring::SubmissionUploads,
        pub color: &'a wgpu::TextureView,
        pub depth: &'a wgpu::TextureView,
        pub guest: (u32, u32),
    }

    type Gpu<'a> = (&'a wgpu::Device, &'a wgpu::Queue, &'a gfx::ring::SubmissionUploads);

    /// One fan: `combiner` with `sampled` stages bound, `quad`'s corners
    /// and taps, into `target`, tested against `depth`, its stages
    /// reading `stages`.
    fn prepare(
        quads: &mut QuadDrawer,
        gpu: Gpu<'_>,
        (combiner, sampled): (&[u32; WORDS], [Option<(u32, u32)>; 4]),
        quad: Quad,
        target: (PostSurface<'_>, wgpu::TextureFormat, PostRaster),
        depth: Option<QuadDepth<'_>>,
        stages: [Option<&wgpu::TextureView>; 4],
    ) -> Result<QuadCommand, &'static str> {
        let program = quads.cached(combiner, sampled, false, FragmentRaster::default()).map_err(|_| "combiner")?;
        quads.prepare_tested(gpu, &program, bytemuck::bytes_of(&quad), None, target, depth, stages)?.ok_or("empty")
    }

    impl FlareDrawGpu {
        pub fn new() -> Self {
            Self::default()
        }

        fn bitmap(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, b: &Bitmap) -> Result<wgpu::TextureView, &'static str> {
            if let Some((key, view)) = &self.bitmap
                && *key == b.key
            {
                return Ok(view.clone());
            }
            let (w, h) = b.size;
            if w == 0 || h == 0 || b.rgba.len() != 4 * w as usize * h as usize {
                return Err("bitmap");
            }
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("native flare glow bitmap"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                texture.as_image_copy(),
                &b.rgba,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) },
                texture.size(),
            );
            let view = texture.create_view(&Default::default());
            self.bitmap = Some((b.key, view.clone()));
            Ok(view)
        }

        fn squares(&mut self, device: &wgpu::Device, side: u32) -> [wgpu::TextureView; 2] {
            match &self.squares {
                Some((s, views)) if *s == side => views.clone(),
                _ => {
                    let views = [square(device, side), square(device, side)];
                    self.squares = Some((side, views.clone()));
                    views
                }
            }
        }

        /// Records `draw` into `encoder` over `t`: the mask's two fans
        /// into the view's alpha (the masked one where the depth holds at
        /// least `reference`, the masked sample's stored depth,
        /// [`crate::flare_gpu::reference`]; above 1 nothing passes), the
        /// down-sample into both squares, the blurs and the composite.
        /// Every pass is prepared before any is recorded; `Err` names why
        /// nothing was.
        pub fn record(
            &mut self,
            t: FlareTarget<'_>,
            encoder: &mut wgpu::CommandEncoder,
            reference: f32,
            draw: &GlowDraw,
            bitmap: &Bitmap,
        ) -> Result<(), &'static str> {
            let ct = t.color.texture();
            let format = ct.format();
            if !crate::glow::live_colour(t.color) || !crate::glow::sampleable(t.device, t.color) {
                return Err("colour");
            }
            let dt = t.depth.texture();
            if !dt.format().has_depth_aspect() || dt.size() != ct.size() || dt.sample_count() != 1 {
                return Err("depth");
            }
            let [x0, y0, x1, y1] = draw.glow.rect;
            if draw.glow.rect.iter().any(|v| !v.is_finite()) || x1 <= x0 || y1 <= y0 || t.guest.0 == 0 || t.guest.1 == 0 {
                return Err("outside");
            }
            let side = side(draw.source, t.guest, (ct.width(), ct.height()));
            let bitmap = self.bitmap(t.device, t.queue, bitmap)?;
            let squares = self.squares(t.device, side);

            let view = PostSurface { view: t.color, guest_size: t.guest };
            let on_square = |n: usize| PostSurface { view: &squares[n], guest_size: (side, side) };
            let raster = |blend, writes| PostRaster { blend, writes, ..PostRaster::default() };
            let alpha = raster(None, wgpu::ColorWrites::ALPHA);
            let gpu = (t.device, t.queue, t.uploads);
            let quads = &mut self.quads;
            let mut pass = |combiner: &[u32; WORDS], sampled, quad, target, depth, stages| {
                prepare(quads, gpu, (combiner, sampled), quad, target, depth, stages)
            };

            let mut commands = Vec::with_capacity(4 + BLURS as usize + draw.rings.len());
            let rect = draw.glow.rect;
            commands.push(pass(&CLEAR, [None; 4], over(rect, t.guest, 0.0, 0.0), (view, format, alpha), None, [None; 4])?);
            let tested = QuadDepth { view: t.depth, compare: wgpu::CompareFunction::LessEqual };
            commands.push(pass(
                &MASKED,
                [UNIT, None, None, None],
                over(rect, t.guest, reference, 0.0),
                (view, format, alpha),
                Some(tested),
                [Some(&bitmap), None, None, None],
            )?);
            let copy = raster(None, wgpu::ColorWrites::ALL);
            for n in 0..2 {
                commands.push(pass(
                    &DOWN,
                    [UNIT, UNIT, None, None],
                    down(rect, t.guest, side),
                    (on_square(n), SQUARE_FORMAT, copy),
                    None,
                    [Some(t.color), Some(&bitmap), None, None],
                )?);
            }
            let by_dest_alpha = raster(blend(wgpu::BlendFactor::DstAlpha, wgpu::BlendFactor::Zero), wgpu::ColorWrites::COLOR);
            for k in 0..BLURS as usize {
                let from = &squares[k % 2];
                commands.push(pass(
                    &blur(k as u32),
                    [UNIT; 4],
                    blurred(side),
                    (on_square((k + 1) % 2), SQUARE_FORMAT, by_dest_alpha),
                    None,
                    [Some(from); 4],
                )?);
            }
            let result = &squares[BLURS as usize % 2];
            let additive = raster(blend(wgpu::BlendFactor::SrcAlpha, wgpu::BlendFactor::One), wgpu::ColorWrites::COLOR);
            for ring in &draw.rings {
                commands.push(pass(
                    &COMPOSITE,
                    [UNIT, None, None, None],
                    over(ring.rect, t.guest, 0.0, ring.weight),
                    (view, format, additive),
                    None,
                    [Some(result), None, None, None],
                )?);
            }
            for c in &commands {
                c.record(encoder);
            }
            Ok(())
        }
    }
}

#[cfg(all(test, feature = "gpu"))]
mod tests {
    use super::*;
    use shaders::psh::interp::{self, Inputs, Sampler, Vec4};
    use shaders::psh::{PixelOptions, PixelShaderDef, PixelUniforms, TextureDim};

    #[test]
    fn the_down_sample_is_the_guest_square_times_the_render_scale() {
        assert_eq!(side([0.0, 64.0], (640, 480), (640, 480)), 64);
        assert_eq!(side([0.53125, 64.53125], (640, 480), (1920, 1440)), 192);
        assert_eq!(side([0.0, 0.0], (640, 480), (640, 480)), 1);
    }

    /// Each stage returns one texel wherever it is sampled.
    struct Texels([Vec4; 4]);

    impl Sampler for Texels {
        fn sample(&self, stage: usize, _: TextureDim, _: [f32; 3]) -> Vec4 {
            self.0[stage]
        }
    }

    /// The block run on one fragment, its stages reading `texels` and
    /// `v0` the diffuse, as the generated program runs it.
    fn run(block: &[u32; WORDS], texels: [Vec4; 4], v0: Vec4) -> Vec4 {
        let def = PixelShaderDef::from_words(block);
        let sampled = |n: u32| (def.texture_modes >> (5 * n)) & 0x1f != 0;
        let options = PixelOptions {
            texture_dims: std::array::from_fn(|n| sampled(n as u32).then_some(TextureDim::D2)),
            ..Default::default()
        };
        let (c0, c1) = def.embedded_constants();
        let u = PixelUniforms { c0, c1, fog: shaders::psh::fog::Fog::OFF.uniform(), ..Default::default() };
        let inputs = Inputs { d0: v0, front: true, ..Default::default() };
        interp::run(&def, &options, &inputs, &u, &Texels(texels)).expect("not killed")
    }

    fn close(actual: &[f32], expected: &[f32]) {
        assert_eq!(actual.len(), expected.len());
        for (a, e) in actual.iter().zip(expected) {
            assert!((a - e).abs() < 1e-5, "{actual:?}, expected {expected:?}");
        }
    }

    fn grey(v: f32) -> Vec4 {
        [v, v, v, 1.0]
    }

    /// The guest's blocks against the combiner worked by hand (the NV2A's
    /// register combiners: inputs clamped by their mapping, outputs to
    /// `[-1, 1]`, the final combiner's to `[0, 1]`).
    #[test]
    fn the_glow_combiners_give_the_hand_worked_values() {
        let none = [[0.0; 4]; 4];
        // The mask fan: alpha 0 (its colour is masked off).
        assert_eq!(run(&CLEAR, none, [0.0; 4])[3], 0.0);
        // The masked sample: 4 a, held at 1.
        let bitmap = |a: f32| [[0.3, 0.6, 0.9, a], [0.0; 4], [0.0; 4], [0.0; 4]];
        close(&run(&MASKED, bitmap(0.1), [0.0; 4]), &[0.4; 4]);
        assert_eq!(run(&MASKED, bitmap(0.3), [0.0; 4])[3], 1.0);
        // The down-sample: the view's alpha (the mask) as colour, the
        // bitmap's alpha times 2 * 192/255 as alpha.
        let view = [0.2, 0.4, 0.6, 0.75];
        let out = run(&DOWN, [view, [0.0, 0.0, 0.0, 0.5], [0.0; 4], [0.0; 4]], [0.0; 4]);
        close(&out, &[0.75, 0.75, 0.75, 192.0 / 255.0]);
        let out = run(&DOWN, [view, [0.0, 0.0, 0.0, 0.9], [0.0; 4], [0.0; 4]], [0.0; 4]);
        assert_eq!(out[3], 1.0, "held at 1");
        // The blur: r0.a = (0.2 + 0.4) / 2 from the first two taps' blue,
        // r0.rgb = (0.1 + 0.3) / 2, their sum times c0.a: 1 on the first
        // pass, 127/255 after. Its alpha is not written.
        let taps = [grey(0.2), grey(0.4), grey(0.1), grey(0.3)];
        close(&run(&blur(0), taps, [0.0; 4])[..3], &[0.5; 3]);
        close(&run(&blur(1), taps, [0.0; 4])[..3], &[0.5 * 127.0 / 255.0; 3]);
        close(&run(&blur(3), taps, [0.0; 4])[..3], &[0.5 * 127.0 / 255.0; 3]);
        assert_eq!(run(&blur(0), [grey(0.9); 4], [0.0; 4])[..3], [1.0; 3], "held at 1");
        // The composite at a mask of 0.75: r0.a = 0.5, half c0 and half
        // white, times the mask; alpha the ring's weight.
        let c0 = [176.0 / 255.0, 176.0 / 255.0, 128.0 / 255.0];
        let lerp = c0.map(|c| (0.5 * c + 0.5) * 0.75);
        close(&run(&COMPOSITE, [grey(0.75), [0.0; 4], [0.0; 4], [0.0; 4]], [0.0, 0.0, 0.0, 0.25]), &[lerp[0], lerp[1], lerp[2], 0.25]);
        // Below half the mask r0.a is negative: c0 alone, times the mask.
        let dim = c0.map(|c| c * 0.25);
        close(&run(&COMPOSITE, [grey(0.25), [0.0; 4], [0.0; 4], [0.0; 4]], [0.0, 0.0, 0.0, 1.0]), &[dim[0], dim[1], dim[2], 1.0]);
    }

    /// The quads put each stage where the guest's texture matrices do: the
    /// view's fans over `[0, 1]` of the rectangle, the down-sample's view
    /// stage over the rectangle in the view's normalised coordinates, the
    /// blur's taps half a texel of 64 off on the diagonals.
    #[test]
    fn the_quads_place_the_stages_as_the_guest_matrices() {
        let coord = |q: &crate::glow::Quad, n: usize, p: [f32; 2]| {
            let t = q.taps[n];
            [(p[0] + t[0]) * t[2], (p[1] + t[1]) * t[3]]
        };
        let rect = [288.0, 208.0, 352.0, 272.0];
        let fan = gpu::over(rect, (640, 480), 0.5, 0.25);
        assert_eq!(fan.dest, [2.0 / 640.0, 2.0 / 480.0, 0.5, 0.25]);
        assert_eq!(coord(&fan, 0, [288.0, 208.0]), [0.0, 0.0]);
        assert_eq!(coord(&fan, 0, [352.0, 272.0]), [1.0, 1.0]);
        // At render scale 3 the square is 192 texels; its texel centres
        // land on the view's guest pixel thirds.
        let down = gpu::down(rect, (640, 480), 192);
        assert_eq!(down.corners, [0.0, 0.0, 192.0, 192.0]);
        let at = coord(&down, 0, [0.0, 0.0]);
        assert!((at[0] - 288.0 / 640.0).abs() < 1e-6 && (at[1] - 208.0 / 480.0).abs() < 1e-6, "{at:?}");
        let at = coord(&down, 0, [192.0, 192.0]);
        assert!((at[0] - 352.0 / 640.0).abs() < 1e-6 && (at[1] - 272.0 / 480.0).abs() < 1e-6, "{at:?}");
        let at = coord(&down, 1, [96.0, 192.0]);
        assert!((at[0] - 0.5).abs() < 1e-6 && (at[1] - 1.0).abs() < 1e-6, "{at:?}");
        let blur = gpu::blurred(64);
        let centre = [32.5, 32.5];
        let base = 32.5 / 64.0;
        let h = 1.0 / 128.0;
        assert_eq!(coord(&blur, 0, centre), [base - h, base - h]);
        assert_eq!(coord(&blur, 1, centre), [base + h, base + h]);
        assert_eq!(coord(&blur, 2, centre), [base - h, base + h]);
        assert_eq!(coord(&blur, 3, centre), [base + h, base - h]);
    }
}
