//! Native text, the GPU side (docs/graphics/native-renderer.md, "Native
//! text"): a batch of screen quads over the glyph cache
//! ([`crate::hud::Batch`]) drawn from a mirror of the cache's texels with
//! the begin's combiner through the stage 3 compiler (`shaders::psh`).
//!
//! A batch is drawn in segments. The host reserves a segment (one external
//! pass) at a quad before it skips that quad; the pass is recorded then,
//! drawing the quads the segment holds through an indirect draw whose
//! count and vertices later quads extend ([`Segment::append`]) through the
//! queue. Those writes land at the next submission, ahead of the pass's
//! commands, so they are drawn as long as no submission came between the
//! reservation and the write: the host only extends a segment while the
//! device has done nothing since it reserved it, and reserves a new one
//! otherwise (`boot::native_hud_glue`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use shaders::psh::{self, PixelOptions, PixelShaderDef, PixelUniforms, TextureDim};

use crate::hud::{FAN_INDICES, GLYPH_SIZE, TextVertex};

/// Quads one segment holds.
pub const SEGMENT_QUADS: u32 = 128;

const QUAD_BYTES: u64 = 4 * std::mem::size_of::<TextVertex>() as u64;

/// The combiner a batch draws with, compiled for the glyph cache at
/// stage 0 (coordinates normalised on the CPU: `tex_scale` one), fog off.
#[derive(Clone, Debug)]
pub struct TextProgram {
    /// `vs_main` ([`TEXT_VERTEX`]) and `fs_main`.
    pub wgsl: String,
    pub pixel: PixelUniforms,
    /// Whether the combiner reads stage 0.
    pub samples: bool,
}

/// `def` (the guest's combiner as the D3D path builds it,
/// `d3d::nv2a::guest_def` with the default final combiner) for a text
/// batch; or why it stays on D3D.
pub fn text_program(def: &PixelShaderDef) -> Result<TextProgram, String> {
    let options = PixelOptions {
        texture_dims: [Some(TextureDim::D2), None, None, None],
        ..Default::default()
    };
    let ps = psh::translate(def, &options);
    if !ps.warnings.is_empty() {
        return Err(format!("combiner: {}", ps.warnings.join("; ")));
    }
    for (n, d) in ps.textures.iter().enumerate() {
        match (n, d) {
            (_, None) | (0, Some(TextureDim::D2)) => {}
            (n, Some(d)) => return Err(format!("stage {n} is fetched as {d:?}")),
        }
    }
    let (c0, c1) = def.embedded_constants();
    Ok(TextProgram {
        wgsl: format!("{}{}", ps.module(), TEXT_VERTEX),
        pixel: PixelUniforms {
            c0,
            c1,
            fog: psh::fog::Fog::OFF.uniform(),
            ..Default::default()
        },
        samples: ps.textures[0].is_some(),
    })
}

/// The text quads' vertex stage: each corner's window coordinate to clip
/// space as the D3D path's translated programs take `oPos`
/// (`shaders::vsh`, `xvs_fixup`): held to the rasterizer's 1/16 at the
/// guest's size; at a render scale shifted (`frame.surface.z`,
/// `gfx::renderer::window_shift`) so an edge on a pixel centre covers
/// whole blocks, and a position past the last pixel centre held on the
/// far edge. Depth is off for the batch, so z is 0 and w 1.
pub const TEXT_VERTEX: &str = r#"
struct TextFrame {
    surface: vec4f,
}
@group(0) @binding(0) var<uniform> frame: TextFrame;

struct TextIn {
    @location(0) window: vec2f,
    @location(1) coords: vec2f,
    @location(2) color: vec4f,
}

@vertex
fn vs_main(v: TextIn) -> XboxVaryings {
    let shift = frame.surface.z;
    let scaled = shift > 0.0;
    let size = frame.surface.xy;
    let t = select(trunc(v.window * 16.0) / 16.0, v.window, scaled);
    let far = vec2<bool>(scaled) & (t > size - 0.5);
    let xy = select(t - shift, max(t - shift, size), far);
    var o: XboxVaryings;
    o.position = vec4f(2.0 * xy.x / size.x - 1.0, 1.0 - 2.0 * xy.y / size.y, 0.0, 1.0);
    o.d0 = v.color;
    o.d1 = vec4f(0.0);
    o.b0 = v.color;
    o.b1 = vec4f(0.0);
    o.t0 = vec4f(v.coords, 0.0, 1.0);
    o.t1 = vec4f(0.0, 0.0, 0.0, 1.0);
    o.t2 = vec4f(0.0, 0.0, 0.0, 1.0);
    o.t3 = vec4f(0.0, 0.0, 0.0, 1.0);
    o.fog = 1.0;
    return o;
}
"#;

const TEXT_ATTRIBUTES: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![
    0 => Float32x2,
    1 => Float32x2,
    2 => Float32x4,
];

/// The pipeline state a batch draws with besides its program: the
/// device's blend, colour writes and cull as a draw would take them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextState {
    pub blend: Option<wgpu::BlendState>,
    pub writes: wgpu::ColorWrites,
    /// Triangle list (the fan's two triangles, same winding), cull and
    /// front face from the device; never unclipped depth.
    pub primitive: wgpu::PrimitiveState,
    /// Point filtering (the record's `+0x96`), else linear; always clamp.
    pub point: bool,
}

/// One segment of a batch: what it draws and the buffers later quads
/// extend.
pub struct TextDraw {
    pub program: TextProgram,
    pub state: TextState,
    /// `D3DRS_BLENDCOLOR` as RGBA.
    pub blend_constant: wgpu::Color,
    /// The glyph cache's texels.
    pub atlas: Arc<Atlas>,
    /// The clip area in guest pixels `(x, y, width, height)`: the
    /// viewport cut by the scissors (`d3d::Device::clip_area`).
    pub clip: [u32; 4],
    /// The quad the segment is reserved at.
    pub first: [TextVertex; 4],
    pub segment: Arc<Segment>,
}

/// A mirror of the glyph cache's texels (RGBA8, `GLYPH_SIZE` square),
/// shared by the segments that read the same texels: its staging buffer
/// and texture are made once, and each recording copies one into the
/// other through its own encoder.
pub struct Atlas {
    pub rgba: Vec<u8>,
    gpu: Mutex<Option<(wgpu::Buffer, wgpu::TextureView)>>,
}

impl Atlas {
    pub fn new(rgba: Vec<u8>) -> Self {
        Self {
            rgba,
            gpu: Mutex::new(None),
        }
    }

    /// Its staging buffer and texture on `device`, made on first use.
    fn gpu(&self, device: &wgpu::Device) -> Option<(wgpu::Buffer, wgpu::TextureView)> {
        let side = GLYPH_SIZE as usize;
        if self.rgba.len() != 4 * side * side {
            return None;
        }
        let mut gpu = self.gpu.lock().unwrap_or_else(|e| e.into_inner());
        if gpu.is_none() {
            use wgpu::util::DeviceExt;
            let staging = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("native text atlas staging"),
                contents: &self.rgba,
                usage: wgpu::BufferUsages::COPY_SRC,
            });
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("native text atlas"),
                size: wgpu::Extent3d {
                    width: GLYPH_SIZE,
                    height: GLYPH_SIZE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            *gpu = Some((staging, texture.create_view(&Default::default())));
        }
        gpu.clone()
    }
}

/// A segment's buffers, made when its pass is first recorded and shared
/// by every copy's recording.
#[derive(Default)]
pub struct Segment {
    inner: Mutex<SegmentInner>,
}

#[derive(Default)]
struct SegmentInner {
    queue: Option<wgpu::Queue>,
    vertices: Option<wgpu::Buffer>,
    indices: Option<wgpu::Buffer>,
    args: Option<wgpu::Buffer>,
    quads: u32,
}

/// The indexed indirect arguments for `quads` quads.
pub fn indirect_args(quads: u32) -> [u32; 5] {
    [6 * quads, 1, 0, 0, 0]
}

/// The index list of `quads` fans.
pub fn fan_indices(quads: u32) -> Vec<u32> {
    (0..quads)
        .flat_map(|q| FAN_INDICES.map(|i| 4 * q + i))
        .collect()
}

impl Segment {
    pub fn new() -> Self {
        Self::default()
    }

    /// Quads drawn so far (0 before the pass is recorded).
    pub fn quads(&self) -> u32 {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).quads
    }

    /// Makes the recorded pass draw nothing and takes no more quads: for a
    /// reservation the host gives up after it was committed, before any
    /// submission (the write lands ahead of the pass's commands).
    pub fn void(&self) {
        let mut s = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let (Some(queue), Some(args)) = (&s.queue, &s.args) {
            queue.write_buffer(args, 0, bytemuck::cast_slice(&indirect_args(0)));
        }
        s.quads = SEGMENT_QUADS;
    }

    /// Adds `quad` to the segment's draw: false (and nothing written) when
    /// the pass was not recorded or the segment is full.
    pub fn append(&self, quad: &[TextVertex; 4]) -> bool {
        let mut s = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if s.quads == 0 || s.quads >= SEGMENT_QUADS {
            return false;
        }
        let (Some(queue), Some(vertices), Some(args)) = (&s.queue, &s.vertices, &s.args) else {
            return false;
        };
        queue.write_buffer(
            vertices,
            u64::from(s.quads) * QUAD_BYTES,
            bytemuck::cast_slice(quad),
        );
        queue.write_buffer(args, 0, bytemuck::cast_slice(&indirect_args(s.quads + 1)));
        s.quads += 1;
        true
    }
}

/// What a segment's pass records into: one copy of the guest's colour
/// target (`d3d::ExternalTarget`'s fields the pass reads).
pub struct TextTarget<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub color: &'a wgpu::TextureView,
    pub color_format: wgpu::TextureFormat,
    /// The target in guest pixels and in this copy's texels.
    pub guest_size: (u32, u32),
    pub size: (u32, u32),
}

struct Compiled {
    module: wgpu::ShaderModule,
    uniforms_layout: wgpu::BindGroupLayout,
    textures_layout: wgpu::BindGroupLayout,
    layout: wgpu::PipelineLayout,
    pipelines: HashMap<(wgpu::TextureFormat, TextState), wgpu::RenderPipeline>,
}

/// Compiled programs (by WGSL) and their pipelines, and the samplers.
#[derive(Default)]
pub struct TextGpu {
    compiled: HashMap<String, Compiled>,
    samplers: HashMap<bool, wgpu::Sampler>,
}

/// `(x, y, width, height)` in guest pixels to this copy's texels, within
/// `size`.
pub fn scale_rect(rect: [u32; 4], guest: (u32, u32), size: (u32, u32)) -> [u32; 4] {
    let sx = |v: u32| (u64::from(v) * u64::from(size.0) / u64::from(guest.0.max(1))) as u32;
    let sy = |v: u32| (u64::from(v) * u64::from(size.1) / u64::from(guest.1.max(1))) as u32;
    let x0 = sx(rect[0]).min(size.0);
    let y0 = sy(rect[1]).min(size.1);
    let x1 = sx(rect[0].saturating_add(rect[2])).min(size.0);
    let y1 = sy(rect[1].saturating_add(rect[3])).min(size.1);
    [x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0)]
}

/// The shift a target `size` texels across takes for a guest `guest`
/// pixels across (`gfx::renderer::window_shift`): 0 at the guest's size.
pub fn shift(guest: u32, size: u32) -> f32 {
    gfx::renderer::window_shift(guest, size)
}

impl TextGpu {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records `draw` into `t`: the atlas copy, then one render pass of
    /// the segment's indirect draw. False (nothing usable recorded) when
    /// the target cannot take it.
    pub fn record(&mut self, draw: &TextDraw, t: TextTarget<'_>) -> bool {
        if t.guest_size.0 == 0 || t.guest_size.1 == 0 || t.size.0 == 0 || t.size.1 == 0 {
            return false;
        }
        let device = t.device;
        let Some((staging, atlas)) = draw.atlas.gpu(device) else {
            return false;
        };
        let mut s = draw.segment.inner.lock().unwrap_or_else(|e| e.into_inner());
        if s.vertices.is_none() {
            use wgpu::util::DeviceExt;
            let mut bytes = vec![0u8; (u64::from(SEGMENT_QUADS) * QUAD_BYTES) as usize];
            bytes[..QUAD_BYTES as usize].copy_from_slice(bytemuck::cast_slice(&draw.first));
            s.vertices = Some(
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("native text vertices"),
                    contents: &bytes,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                }),
            );
            s.indices = Some(
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("native text indices"),
                    contents: bytemuck::cast_slice(&fan_indices(SEGMENT_QUADS)),
                    usage: wgpu::BufferUsages::INDEX,
                }),
            );
            s.args = Some(
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("native text draw"),
                    contents: bytemuck::cast_slice(&indirect_args(1)),
                    usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
                }),
            );
            s.queue = Some(t.queue.clone());
            s.quads = 1;
        }
        let (Some(vertices), Some(indices), Some(args)) = (&s.vertices, &s.indices, &s.args) else {
            return false;
        };
        // The texels go in through this encoder only (a recording that is
        // cancelled leaves nothing behind but its own texture).
        t.encoder.copy_buffer_to_texture(
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * GLYPH_SIZE),
                    rows_per_image: Some(GLYPH_SIZE),
                },
            },
            atlas.texture().as_image_copy(),
            wgpu::Extent3d {
                width: GLYPH_SIZE,
                height: GLYPH_SIZE,
                depth_or_array_layers: 1,
            },
        );
        let point = draw.state.point;
        let sampler = self
            .samplers
            .entry(point)
            .or_insert_with(|| {
                let filter = if point {
                    wgpu::FilterMode::Nearest
                } else {
                    wgpu::FilterMode::Linear
                };
                device.create_sampler(&wgpu::SamplerDescriptor {
                    label: Some("native text"),
                    address_mode_u: wgpu::AddressMode::ClampToEdge,
                    address_mode_v: wgpu::AddressMode::ClampToEdge,
                    address_mode_w: wgpu::AddressMode::ClampToEdge,
                    mag_filter: filter,
                    min_filter: filter,
                    mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                    lod_max_clamp: 0.0,
                    ..Default::default()
                })
            })
            .clone();
        let c = self
            .compiled
            .entry(draw.program.wgsl.clone())
            .or_insert_with(|| compile(device, &draw.program));
        let format = t.color_format;
        let state = draw.state;
        if !c.pipelines.contains_key(&(format, state)) {
            let p = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("native text"),
                layout: Some(&c.layout),
                vertex: wgpu::VertexState {
                    module: &c.module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<TextVertex>() as wgpu::BufferAddress,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &TEXT_ATTRIBUTES,
                    })],
                },
                primitive: state.primitive,
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &c.module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: state.blend,
                        write_mask: state.writes,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            });
            c.pipelines.insert((format, state), p);
        }
        let buffer = |label, bytes: &[u8]| {
            use wgpu::util::DeviceExt;
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytes,
                usage: wgpu::BufferUsages::UNIFORM,
            })
        };
        let frame: [f32; 4] = [
            t.guest_size.0 as f32,
            t.guest_size.1 as f32,
            shift(t.guest_size.0, t.size.0),
            0.0,
        ];
        let frame = buffer("native text frame", bytemuck::cast_slice(&frame));
        let pixel = buffer("native text pixel", &draw.program.pixel.to_bytes());
        let uniforms = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native text uniforms"),
            layout: &c.uniforms_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: frame.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: pixel.as_entire_binding(),
                },
            ],
        });
        let mut entries = Vec::new();
        if draw.program.samples {
            entries.push(wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&atlas),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            });
        }
        let textures = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native text textures"),
            layout: &c.textures_layout,
            entries: &entries,
        });
        let [x, y, w, h] = scale_rect(draw.clip, t.guest_size, t.size);
        let mut pass = t.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native text"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: t.color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if w == 0 || h == 0 {
            // The clip keeps nothing: neither does the D3D path.
            return true;
        }
        pass.set_viewport(0.0, 0.0, t.size.0 as f32, t.size.1 as f32, 0.0, 1.0);
        pass.set_scissor_rect(x, y, w, h);
        pass.set_pipeline(&c.pipelines[&(format, state)]);
        pass.set_blend_constant(draw.blend_constant);
        pass.set_bind_group(0, &uniforms, &[]);
        pass.set_bind_group(1, &textures, &[]);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed_indirect(args, 0);
        true
    }
}

fn compile(device: &wgpu::Device, program: &TextProgram) -> Compiled {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("native text"),
        source: wgpu::ShaderSource::Wgsl(program.wgsl.clone().into()),
    });
    let mut entries = Vec::new();
    if program.samples {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        });
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        });
    }
    let textures_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("native text textures"),
        entries: &entries,
    });
    let uniforms_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("native text uniforms"),
        entries: &[
            crate::program::uniform_entry(0, wgpu::ShaderStages::VERTEX),
            crate::program::uniform_entry(1, wgpu::ShaderStages::FRAGMENT),
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("native text"),
        bind_group_layouts: &[Some(&uniforms_layout), Some(&textures_layout)],
        immediate_size: 0,
    });
    Compiled {
        module,
        uniforms_layout,
        textures_layout,
        layout,
        pipelines: HashMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_segment_draws_six_indices_per_quad() {
        assert_eq!(indirect_args(1), [6, 1, 0, 0, 0]);
        assert_eq!(indirect_args(3)[0], 18);
        assert_eq!(fan_indices(2), vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]);
    }

    #[test]
    fn a_segment_not_recorded_takes_no_quads() {
        let s = Segment::new();
        let q = [TextVertex {
            window: [0.0; 2],
            coords: [0.0; 2],
            color: [0.0; 4],
        }; 4];
        assert!(!s.append(&q));
        assert_eq!(s.quads(), 0);
    }

    #[test]
    fn the_clip_scales_to_the_copys_texels() {
        assert_eq!(
            scale_rect([10, 20, 100, 50], (640, 480), (640, 480)),
            [10, 20, 100, 50]
        );
        assert_eq!(
            scale_rect([10, 20, 100, 50], (640, 480), (1280, 960)),
            [20, 40, 200, 100]
        );
        assert_eq!(
            scale_rect([600, 0, 100, 10], (640, 480), (640, 480)),
            [600, 0, 40, 10]
        );
        assert_eq!(scale_rect([700, 0, 10, 10], (640, 480), (640, 480))[2], 0);
    }

    #[test]
    fn the_shift_is_zero_at_the_guests_size() {
        assert_eq!(shift(640, 640), 0.0);
        assert!(shift(640, 1280) > 0.0);
    }

    #[test]
    fn a_combiner_that_samples_stage_0_compiles_with_the_text_vertex_stage() {
        // Stage 0 PROJECT2D: a synthetic block.
        let def = PixelShaderDef {
            texture_modes: 1,
            ..Default::default()
        };
        let p = text_program(&def).expect("compiles");
        shaders::validate(&p.wgsl).expect("valid WGSL");
        assert!(
            p.wgsl.contains("fn vs_main(v: TextIn) -> XboxVaryings")
                && p.wgsl.contains("fn fs_main")
        );
        assert!(p.samples);
        assert_eq!(p.pixel.fog, psh::fog::Fog::OFF.uniform());
        assert_eq!(p.pixel.tex_scale[0], [1.0; 4], "normalised on the CPU");
        let none = text_program(&PixelShaderDef::default()).expect("compiles");
        shaders::validate(&none.wgsl).expect("valid WGSL");
        assert!(!none.samples);
    }
}
