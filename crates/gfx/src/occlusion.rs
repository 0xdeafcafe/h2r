//! Counting occlusion queries: how many samples a run of draws wrote.
//!
//! The NV2A keeps one running "zpass" pixel counter. `CLEAR_REPORT_VALUE`
//! zeroes it, draws issued while `SET_ZPASS_PIXEL_COUNT_ENABLE` is on add
//! the samples that pass the depth and stencil tests, and `GET_REPORT`
//! writes its current value to memory. Halo divides that count by the one
//! it expects, so it has to be a real count.
//!
//! wgpu occlusion queries are not one: WebGPU only promises zero or
//! non-zero, and the Metal backend runs them in Metal's boolean mode. So the
//! count is taken by drawing again. For each counted draw
//! ([`Draw::count_samples`](crate::Draw::count_samples)),
//! [`Renderer::render_counted`](crate::Renderer::render_counted), *before*
//! the real draw:
//!
//! 1. draws the same geometry with the same vertex stage, primitive,
//!    depth/stencil test and scissor into a cleared `R16Float` count
//!    texture, with depth and stencil writes off, additive blending and a
//!    fragment stage that writes 1. Each texel then holds the number of
//!    the draw's fragments that passed there (exact up to 2048 per texel);
//! 2. sums the texels inside the scissor rectangle with a compute pass into
//!    the counter's 32-bit total.
//!
//! Then the real draw runs and changes depth and stencil as it should.
//! What is not reproduced: fragment-stage rejection (alpha test, alpha
//! kill, `discard`), because the count pass does not run the draw's
//! fragment program. Every sample that survives the fixed-function tests is
//! counted.
//!
//! [`PixelCounter::clear`] and [`PixelCounter::report`] record into the
//! caller's encoder, so they stay in order with the counted passes;
//! [`PixelReport::read`] waits for the GPU and must only be called once
//! that encoder has been submitted.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use wgpu::util::DeviceExt;

use crate::context::GfxContext;

/// Format of the per-texel count.
pub const COUNT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;

const SUM_WGSL: &str = r#"
struct Rect { x: u32, y: u32, w: u32, h: u32 }
@group(0) @binding(0) var counts: TEXTURE;
@group(0) @binding(1) var<storage, read_write> total: atomic<u32>;
@group(0) @binding(2) var<uniform> rect: Rect;
var<workgroup> partial: atomic<u32>;

@compute @workgroup_size(8, 8)
fn sum(@builtin(global_invocation_id) id: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    if (li == 0u) {
        atomicStore(&partial, 0u);
    }
    workgroupBarrier();
    if (id.x < rect.w && id.y < rect.h) {
        let p = vec2<u32>(rect.x + id.x, rect.y + id.y);
        var n = 0u;
        LOAD
        if (n != 0u) {
            atomicAdd(&partial, n);
        }
    }
    workgroupBarrier();
    if (li == 0u) {
        let v = atomicLoad(&partial);
        if (v != 0u) {
            atomicAdd(&total, v);
        }
    }
}
"#;

const SINGLE_LOAD: &str = "n = u32(textureLoad(counts, p, 0).r + 0.5);";
const MULTI_LOAD: &str = "for (var s = 0u; s < textureNumSamples(counts); s++) { n += u32(textureLoad(counts, p, i32(s)).r + 0.5); }";

/// The fragment stage of the count pass: one per fragment that gets this
/// far.
pub(crate) const COUNT_FRAGMENT_WGSL: &str =
    "@fragment fn fs_count() -> @location(0) vec4<f32> { return vec4<f32>(1.0, 0.0, 0.0, 0.0); }";

/// The samples counted between a clear and a report. Cheap to clone; the
/// GPU value is read once.
#[derive(Clone)]
pub struct PixelReport {
    buffer: Arc<wgpu::Buffer>,
    value: Arc<Mutex<Option<u64>>>,
}

impl PixelReport {
    /// Samples that passed, summed over every counted draw since the clear.
    /// Blocks until the GPU has finished; the encoder the report was
    /// recorded into must have been submitted.
    pub fn read(&self, ctx: &GfxContext) -> Result<u64, String> {
        let mut v = self.value.lock().map_err(|_| "pixel count poisoned")?;
        if let Some(v) = *v {
            return Ok(v);
        }
        let slice = self.buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        ctx.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| format!("waiting for the pixel count: {e}"))?;
        rx.recv()
            .map_err(|_| "pixel-count readback dropped".to_owned())?
            .map_err(|e| format!("mapping the pixel count: {e}"))?;
        let count = {
            let data = slice
                .get_mapped_range()
                .map_err(|e| format!("pixel count: {e}"))?;
            u32::from_le_bytes(data[..4].try_into().expect("4 bytes")) as u64
        };
        self.buffer.unmap();
        *v = Some(count);
        Ok(count)
    }
}

impl std::fmt::Debug for PixelReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PixelReport")
            .field("value", &self.value.lock().ok().and_then(|v| *v))
            .finish()
    }
}

/// The running sample counter (see the module docs).
pub struct PixelCounter {
    total: wgpu::Buffer,
    single: (wgpu::BindGroupLayout, wgpu::ComputePipeline),
    multi: (wgpu::BindGroupLayout, wgpu::ComputePipeline),
    /// Count textures by `(width, height, samples)`.
    targets: HashMap<(u32, u32, u32), (wgpu::Texture, wgpu::TextureView)>,
}

fn sum_pipeline(
    device: &wgpu::Device,
    multisampled: bool,
) -> (wgpu::BindGroupLayout, wgpu::ComputePipeline) {
    let (texture, load) = if multisampled {
        ("texture_multisampled_2d<f32>", MULTI_LOAD)
    } else {
        ("texture_2d<f32>", SINGLE_LOAD)
    };
    let source = SUM_WGSL.replace("TEXTURE", texture).replace("LOAD", load);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("gfx pixel count sum"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("gfx pixel count sum"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(4),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(16),
                },
                count: None,
            },
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("gfx pixel count sum"),
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("gfx pixel count sum"),
        layout: Some(&layout),
        module: &module,
        entry_point: Some("sum"),
        compilation_options: Default::default(),
        cache: None,
    });
    (bgl, pipeline)
}

impl PixelCounter {
    /// A counter at zero.
    pub fn new(ctx: &GfxContext) -> Self {
        let total = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gfx pixel count"),
            size: 4,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            total,
            single: sum_pipeline(&ctx.device, false),
            multi: sum_pipeline(&ctx.device, true),
            targets: HashMap::new(),
        }
    }

    /// `CLEAR_REPORT_VALUE`: the count restarts at zero once `encoder`
    /// runs.
    pub fn clear(&mut self, encoder: &mut wgpu::CommandEncoder) {
        encoder.clear_buffer(&self.total, 0, None);
    }

    /// `GET_REPORT`: the samples counted since the last clear, as of this
    /// point in `encoder`. Read it after the encoder is submitted.
    pub fn report(&mut self, ctx: &GfxContext, encoder: &mut wgpu::CommandEncoder) -> PixelReport {
        let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gfx pixel count report"),
            size: 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_buffer_to_buffer(&self.total, 0, &buffer, 0, 4);
        PixelReport {
            buffer: Arc::new(buffer),
            value: Arc::new(Mutex::new(None)),
        }
    }

    /// The count texture for attachments of `size` with `samples` samples.
    pub(crate) fn target(
        &mut self,
        ctx: &GfxContext,
        size: (u32, u32),
        samples: u32,
    ) -> &wgpu::TextureView {
        &self
            .targets
            .entry((size.0, size.1, samples))
            .or_insert_with(|| {
                let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("gfx pixel count target"),
                    size: wgpu::Extent3d {
                        width: size.0.max(1),
                        height: size.1.max(1),
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format: COUNT_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                });
                let view = texture.create_view(&Default::default());
                (texture, view)
            })
            .1
    }

    /// Adds the texels of the count texture for `(size, samples)` inside
    /// `rect` (`x, y, width, height`, already inside the texture) to the
    /// total.
    pub(crate) fn sum(
        &mut self,
        ctx: &GfxContext,
        encoder: &mut wgpu::CommandEncoder,
        size: (u32, u32),
        samples: u32,
        rect: [u32; 4],
    ) {
        if rect[2] == 0 || rect[3] == 0 {
            return;
        }
        let view = self.target(ctx, size, samples).clone();
        let (bgl, pipeline) = if samples > 1 {
            &self.multi
        } else {
            &self.single
        };
        let uniform = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("gfx pixel count rect"),
                contents: &rect
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect::<Vec<u8>>(),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gfx pixel count sum"),
            layout: bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.total.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("gfx pixel count sum"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(rect[2].div_ceil(8), rect[3].div_ceil(8), 1);
    }
}
