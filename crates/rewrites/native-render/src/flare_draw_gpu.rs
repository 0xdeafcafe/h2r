//! The flare glow `0x00046ea0` drawn by the native renderer
//! (docs/graphics/d3d-visibility.md, "The glow in Rust"). The glow's
//! numbers are `scene_extract::flares::glow`'s; this draws them.
//!
//! | Guest | Here |
//! |---|---|
//! | the mask fan and the masked sample (`0x00047306`, `0x00047512`): the view's alpha over the rectangle, set where a fan at `z = w = 16777215` passes `LESSEQUAL` | the down-sample's test of the depth at each texel, against that fan's stored depth ([`MASK_DEPTH`], `flare_gpu::reference`) |
//! | `0x00046230` into targets `0x10` and `0x11` | one down-sample of the rectangle into a square of [`side`] texels |
//! | `0x000467c0(0x10, 0x11, 4)` | four four-tap blurs, ping-ponging two squares |
//! | the composite's sixteen fans (`0x00047854`), additive (`SRCALPHA`, `ONE`), colour only | sixteen instances over the view's colour, ring `i`'s weight its alpha |
//!
//! Limits: the masked sample's texture (`[[0x00485a80] + 0x34]`), the
//! combiner blocks the guest writes at `0x00484f68` for each fan, and the
//! blur's texture-matrix offsets are not read: the down-sample takes the
//! colour as it is, the blur's taps are a diagonal half texel plus the
//! pass, and the composite adds the blurred colour times the ring's
//! weight. The view's alpha is left as it was.

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

/// The host texels a glow's rectangle covers: `origin` and `size` the
/// whole rectangle's, `inside` (`x0, y0, x1, y1`) its part inside the
/// target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub origin: [i32; 2],
    pub size: [u32; 2],
    pub inside: [u32; 4],
}

/// The largest side a region may have (the half size is a few pixels; a
/// larger one is not the guest's glow).
pub const MAX_REGION: u32 = 4096;

/// `rect` (guest pixels of a target of `guest`) in the host's `host`
/// texels; `None` when no texel of it is inside the target.
pub fn region(rect: [f32; 4], guest: (u32, u32), host: (u32, u32)) -> Option<Region> {
    if guest.0 == 0 || guest.1 == 0 || rect.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let (sx, sy) = (host.0 as f32 / guest.0 as f32, host.1 as f32 / guest.1 as f32);
    let x0 = (rect[0] * sx).floor() as i64;
    let y0 = (rect[1] * sy).floor() as i64;
    let x1 = (rect[2] * sx).ceil() as i64;
    let y1 = (rect[3] * sy).ceil() as i64;
    let (w, h) = (x1 - x0, y1 - y0);
    if w <= 0 || h <= 0 || w > i64::from(MAX_REGION) || h > i64::from(MAX_REGION) {
        return None;
    }
    let clip = |v: i64, hi: u32| v.clamp(0, i64::from(hi)) as u32;
    let inside = [clip(x0, host.0), clip(y0, host.1), clip(x1, host.0), clip(y1, host.1)];
    if inside[2] <= inside[0] || inside[3] <= inside[1] {
        return None;
    }
    Some(Region { origin: [x0 as i32, y0 as i32], size: [w as u32, h as u32], inside })
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

#[cfg(feature = "gpu")]
pub use gpu::FlareDrawGpu;

#[cfg(feature = "gpu")]
mod gpu {
    use std::collections::HashMap;

    use super::{BLURS, GlowDraw, RINGS, region, side};

    const SQUARE: &str = r#"
struct Params { copy: vec4<i32>, depth: vec4<i32>, tap: vec4f };
@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var depth_tex: texture_depth_2d;
@group(0) @binding(2) var smp: sampler;
@group(0) @binding(3) var<uniform> params: Params;

struct Screen { @builtin(position) position: vec4f, @location(0) uv: vec2f };

@vertex
fn vs_screen(@builtin(vertex_index) i: u32) -> Screen {
    let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    var o: Screen;
    o.position = vec4f(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    o.uv = uv;
    return o;
}

// The rectangle's colour where the masked sample's depth (tap.w) passes
// LESSEQUAL against the stored depth, else nothing.
@fragment
fn fs_source(i: Screen) -> @location(0) vec4f {
    let c = textureSampleLevel(src_tex, smp, i.uv, 0.0);
    let p = params.copy.xy + vec2<i32>(i.uv * vec2f(params.copy.zw));
    var seen = 0.0;
    if (all(p >= vec2<i32>(0)) && all(p < params.depth.xy)) {
        seen = select(0.0, 1.0, params.tap.w <= textureLoad(depth_tex, p, 0));
    }
    return vec4f(c.rgb * seen, seen);
}

@fragment
fn fs_blur(i: Screen) -> @location(0) vec4f {
    let o = params.tap.xy * params.tap.z;
    let s = textureSampleLevel(src_tex, smp, i.uv + vec2f(o.x, o.y), 0.0)
        + textureSampleLevel(src_tex, smp, i.uv + vec2f(-o.x, o.y), 0.0)
        + textureSampleLevel(src_tex, smp, i.uv + vec2f(o.x, -o.y), 0.0)
        + textureSampleLevel(src_tex, smp, i.uv - o, 0.0);
    return s * 0.25;
}
"#;

    const COMPOSITE: &str = r#"
struct Rings { rects: array<vec4f, 16>, weights: array<vec4f, 4>, guest: vec4f };
@group(0) @binding(0) var glow_tex: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
@group(0) @binding(2) var<uniform> rings: Rings;

struct Fan { @builtin(position) position: vec4f, @location(0) uv: vec2f, @location(1) weight: f32 };

// Corner v of ring i: a four-vertex strip over the ring's rectangle.
@vertex
fn vs_ring(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> Fan {
    let c = vec2f(f32(v & 1u), f32(v >> 1u));
    let r = rings.rects[i];
    let p = mix(r.xy, r.zw, c);
    var o: Fan;
    o.position = vec4f(p.x * rings.guest.x - 1.0, 1.0 - p.y * rings.guest.y, 0.0, 1.0);
    o.uv = c;
    o.weight = rings.weights[i / 4u][i % 4u];
    return o;
}

@fragment
fn fs_ring(f: Fan) -> @location(0) vec4f {
    let t = textureSampleLevel(glow_tex, smp, f.uv, 0.0);
    return vec4f(t.rgb, f.weight);
}
"#;

    #[repr(C)]
    #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
    struct Params {
        copy: [i32; 4],
        depth: [i32; 4],
        tap: [f32; 4],
    }

    #[repr(C)]
    #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
    struct Rings {
        rects: [[f32; 4]; RINGS],
        weights: [[f32; 4]; RINGS / 4],
        guest: [f32; 4],
    }

    /// The squares' format: filterable and renderable everywhere.
    const SQUARE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

    /// The glow's pipelines on one device.
    pub struct FlareDrawGpu {
        sampler: wgpu::Sampler,
        square_layout: wgpu::BindGroupLayout,
        ring_layout: wgpu::BindGroupLayout,
        ring_module: wgpu::ShaderModule,
        ring_pipeline_layout: wgpu::PipelineLayout,
        source: wgpu::RenderPipeline,
        blur: wgpu::RenderPipeline,
        composite: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    }

    fn texture_entry(binding: u32, sample_type: wgpu::TextureSampleType, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
        wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        }
    }

    fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
        wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        }
    }

    fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
        wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn pipeline(
        device: &wgpu::Device,
        label: &str,
        layout: &wgpu::PipelineLayout,
        module: &wgpu::ShaderModule,
        vertex: &str,
        fragment: &str,
        format: wgpu::TextureFormat,
        topology: wgpu::PrimitiveTopology,
        blend: Option<wgpu::BlendState>,
        write_mask: wgpu::ColorWrites,
    ) -> wgpu::RenderPipeline {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            vertex: wgpu::VertexState { module, entry_point: Some(vertex), compilation_options: Default::default(), buffers: &[] },
            primitive: wgpu::PrimitiveState { topology, cull_mode: None, ..Default::default() },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some(fragment),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask })],
            }),
            multiview_mask: None,
            cache: None,
        })
    }

    /// The composite's blend: `SRCALPHA`, `ONE`, `ADD` on the colour; the
    /// alpha is not written.
    fn additive() -> wgpu::BlendState {
        let colour = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        wgpu::BlendState { color: colour, alpha: wgpu::BlendComponent::REPLACE }
    }

    fn square_texture(device: &wgpu::Device, side: u32) -> wgpu::TextureView {
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

    fn screen_pass<'e>(
        encoder: &'e mut wgpu::CommandEncoder,
        label: &str,
        target: &wgpu::TextureView,
        load: wgpu::LoadOp<wgpu::Color>,
    ) -> wgpu::RenderPass<'e> {
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        })
    }

    impl FlareDrawGpu {
        pub fn new(device: &wgpu::Device) -> Self {
            let fragment = wgpu::ShaderStages::FRAGMENT;
            let float = wgpu::TextureSampleType::Float { filterable: true };
            let square_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("native flare glow square"),
                entries: &[
                    texture_entry(0, float, fragment),
                    texture_entry(1, wgpu::TextureSampleType::Depth, fragment),
                    sampler_entry(2),
                    uniform_entry(3, fragment),
                ],
            });
            let ring_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("native flare glow rings"),
                entries: &[texture_entry(0, float, fragment), sampler_entry(1), uniform_entry(2, wgpu::ShaderStages::VERTEX)],
            });
            let square_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("native flare glow square"),
                source: wgpu::ShaderSource::Wgsl(SQUARE.into()),
            });
            let ring_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("native flare glow rings"),
                source: wgpu::ShaderSource::Wgsl(COMPOSITE.into()),
            });
            let square_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("native flare glow square"),
                bind_group_layouts: &[Some(&square_layout)],
                immediate_size: 0,
            });
            let ring_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("native flare glow rings"),
                bind_group_layouts: &[Some(&ring_layout)],
                immediate_size: 0,
            });
            let list = wgpu::PrimitiveTopology::TriangleList;
            let all = wgpu::ColorWrites::ALL;
            let source = pipeline(device, "native flare glow source", &square_pipeline_layout, &square_module, "vs_screen", "fs_source", SQUARE_FORMAT, list, None, all);
            let blur = pipeline(device, "native flare glow blur", &square_pipeline_layout, &square_module, "vs_screen", "fs_blur", SQUARE_FORMAT, list, None, all);
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("native flare glow"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });
            Self { sampler, square_layout, ring_layout, ring_module, ring_pipeline_layout, source, blur, composite: HashMap::new() }
        }

        /// Records `draw` over `color` (a target of `guest` pixels) into
        /// `encoder`: the down-sample of its rectangle where `depth` holds
        /// at least `reference` (the masked sample's stored depth,
        /// [`crate::flare_gpu::reference`]; above 1 nothing passes), the
        /// blurs and the composite. `Err` names why nothing was recorded.
        pub fn record(
            &mut self,
            device: &wgpu::Device,
            encoder: &mut wgpu::CommandEncoder,
            color: &wgpu::TextureView,
            depth: &wgpu::TextureView,
            guest: (u32, u32),
            reference: f32,
            draw: &GlowDraw,
        ) -> Result<(), &'static str> {
            use wgpu::TextureFormat as F;
            let ct = color.texture();
            let dt = depth.texture();
            let format = ct.format();
            if !crate::glow::live_colour(color)
                || !ct.usage().contains(wgpu::TextureUsages::COPY_SRC)
                || !gfx::program::filterable(format, device.features())
            {
                return Err("colour");
            }
            if !dt.usage().contains(wgpu::TextureUsages::TEXTURE_BINDING)
                || dt.sample_count() != 1
                || !matches!(dt.format(), F::Depth16Unorm | F::Depth24Plus | F::Depth24PlusStencil8 | F::Depth32Float | F::Depth32FloatStencil8)
            {
                return Err("depth");
            }
            let host = (ct.width(), ct.height());
            let r = region(draw.glow.rect, guest, host).ok_or("outside")?;
            let side = side(draw.source, guest, host);

            // The rectangle's colour, as the pass found it (an attachment
            // cannot be sampled as it is drawn into).
            let copy = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("native flare glow rectangle"),
                size: wgpu::Extent3d { width: r.size[0], height: r.size[1], depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let [ix0, iy0, ix1, iy1] = r.inside;
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo { texture: ct, mip_level: 0, origin: wgpu::Origin3d { x: ix0, y: iy0, z: 0 }, aspect: wgpu::TextureAspect::All },
                wgpu::TexelCopyTextureInfo {
                    texture: &copy,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: (ix0 as i32 - r.origin[0]) as u32, y: (iy0 as i32 - r.origin[1]) as u32, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d { width: ix1 - ix0, height: iy1 - iy0, depth_or_array_layers: 1 },
            );
            let copy_view = copy.create_view(&Default::default());
            let depth_view = dt.create_view(&wgpu::TextureViewDescriptor { aspect: wgpu::TextureAspect::DepthOnly, ..Default::default() });
            let squares = [square_texture(device, side), square_texture(device, side)];

            use wgpu::util::DeviceExt;
            let uniform = |label, contents: &[u8]| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage: wgpu::BufferUsages::UNIFORM })
            };
            let texel = 1.0 / side as f32;
            let params = |tap: [f32; 4]| Params {
                copy: [r.origin[0], r.origin[1], r.size[0] as i32, r.size[1] as i32],
                depth: [dt.width() as i32, dt.height() as i32, 0, 0],
                tap,
            };
            let square_group = |from: &wgpu::TextureView, buffer: &wgpu::Buffer| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("native flare glow square"),
                    layout: &self.square_layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(from) },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&depth_view) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                        wgpu::BindGroupEntry { binding: 3, resource: buffer.as_entire_binding() },
                    ],
                })
            };
            let source_params = uniform("native flare glow source", bytemuck::bytes_of(&params([texel, texel, 0.0, reference])));
            let source_group = square_group(&copy_view, &source_params);
            {
                let mut pass = screen_pass(encoder, "native flare glow source", &squares[0], wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT));
                pass.set_pipeline(&self.source);
                pass.set_bind_group(0, &source_group, &[]);
                pass.draw(0..3, 0..1);
            }
            for k in 0..BLURS as usize {
                let (from, to) = (&squares[k % 2], &squares[(k + 1) % 2]);
                let buffer = uniform("native flare glow blur", bytemuck::bytes_of(&params([texel, texel, k as f32 + 0.5, reference])));
                let group = square_group(from, &buffer);
                let mut pass = screen_pass(encoder, "native flare glow blur", to, wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT));
                pass.set_pipeline(&self.blur);
                pass.set_bind_group(0, &group, &[]);
                pass.draw(0..3, 0..1);
            }
            let result = &squares[BLURS as usize % 2];

            let mut rings = Rings { rects: [[0.0; 4]; RINGS], weights: [[0.0; 4]; RINGS / 4], guest: [2.0 / guest.0 as f32, 2.0 / guest.1 as f32, 0.0, 0.0] };
            for (i, ring) in draw.rings.iter().enumerate() {
                rings.rects[i] = ring.rect;
                rings.weights[i / 4][i % 4] = ring.weight;
            }
            let ring_buffer = uniform("native flare glow rings", bytemuck::bytes_of(&rings));
            let ring_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("native flare glow rings"),
                layout: &self.ring_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(result) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    wgpu::BindGroupEntry { binding: 2, resource: ring_buffer.as_entire_binding() },
                ],
            });
            let (module, layout) = (&self.ring_module, &self.ring_pipeline_layout);
            let composite = self.composite.entry(format).or_insert_with(|| {
                pipeline(
                    device,
                    "native flare glow composite",
                    layout,
                    module,
                    "vs_ring",
                    "fs_ring",
                    format,
                    wgpu::PrimitiveTopology::TriangleStrip,
                    Some(additive()),
                    wgpu::ColorWrites::COLOR,
                )
            });
            let mut pass = screen_pass(encoder, "native flare glow composite", color, wgpu::LoadOp::Load);
            pass.set_pipeline(composite);
            pass.set_bind_group(0, &ring_group, &[]);
            pass.draw(0..4, 0..RINGS as u32);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// At render scale 2 a rectangle half off the right edge covers twice
    /// its texels, and only its inside part is copied; one wholly outside
    /// is not drawn.
    #[test]
    fn a_glow_half_off_the_target_copies_what_is_inside() {
        let r = region([632.0, 232.0, 648.0, 248.0], (640, 480), (1280, 960)).unwrap();
        assert_eq!(r.origin, [1264, 464]);
        assert_eq!(r.size, [32, 32]);
        assert_eq!(r.inside, [1264, 464, 1280, 496]);
        assert_eq!(region([700.0, 0.0, 716.0, 16.0], (640, 480), (640, 480)), None);
        assert_eq!(region([0.0, 0.0, f32::NAN, 1.0], (640, 480), (640, 480)), None);
    }

    #[test]
    fn the_down_sample_is_the_guest_square_times_the_render_scale() {
        assert_eq!(side([0.0, 64.0], (640, 480), (640, 480)), 64);
        assert_eq!(side([0.0, 64.0], (640, 480), (1920, 1440)), 192);
        assert_eq!(side([0.0, 0.0], (640, 480), (640, 480)), 1);
    }
}
