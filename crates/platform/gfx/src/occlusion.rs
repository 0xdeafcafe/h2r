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
//! 1. copies the depth/stencil attachment into a scratch texture of the
//!    counter's (the attachment needs `COPY_SRC`), so the replay can change
//!    depth and stencil without touching the real ones;
//! 2. draws the same geometry with the same vertex stage, primitive,
//!    depth/stencil state (writes included) and scissor into a cleared
//!    `R16Float` count texture, against the scratch copy, with additive
//!    blending and a fragment stage that writes 1. Each texel then holds
//!    the number of the draw's fragments that passed there (exact up to
//!    2048 per texel). Because the replay writes depth and stencil as the
//!    draw does, a later triangle of the same draw that the real draw
//!    rejects (behind an earlier one, or failing a stencil the draw just
//!    changed) is rejected here too;
//! 3. sums the texels inside the scissor rectangle with a compute pass into
//!    the counter's 32-bit total.
//!
//! Then the real draw runs against the real attachment, which is written
//! once.
//! What is not reproduced: fragment-stage rejection (alpha test, alpha
//! kill, `discard`), because the count pass does not run the draw's
//! fragment program. Every sample that survives the fixed-function tests is
//! counted.
//!
//! A program whose fragment stage writes the depth (the NV2A's W-buffer,
//! which Halo runs in) names its own count entry
//! ([`ShaderProgram::count_entry`](crate::pipeline::ShaderProgram::count_entry)):
//! it writes the same depth and 1 in red, and the count pass uses it in
//! place of the generic fragment. Otherwise the replay would test the
//! rasterized z against depths that hold w, and count nothing in front of
//! anything drawn.
//!
//! [`PixelCounter::clear`] and [`PixelCounter::report`] record into the
//! caller's encoder, so they stay in order with the counted passes;
//! [`PixelReport::read`] waits for the GPU and must only be called once
//! that encoder has been submitted. Telling the report which submission
//! holds it ([`PixelReport::submitted`]) makes the read wait for that
//! submission only, not for everything submitted since: a report read a
//! frame or two later is then usually ready without waiting.
//!
//! A counted draw that writes neither depth nor stencil replays against
//! the real depth/stencil attachment (it cannot change it), so only a draw
//! that writes them pays for the scratch copy.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use wgpu::util::DeviceExt;

use crate::context::GfxContext;

/// Format of the per-texel count.
pub const COUNT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;

const SUM_WGSL: &str = r#"
struct Rect { x: u32, y: u32, w: u32, h: u32, stride: u32 }
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
        let p = vec2<u32>(rect.x + id.x * rect.stride, rect.y + id.y * rect.stride);
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
    submission: Arc<Mutex<Option<wgpu::SubmissionIndex>>>,
    /// The map started by [`try_read`](Self::try_read): its outcome
    /// arrives here once the GPU is done with the buffer.
    mapping: Arc<Mutex<Option<MapOutcome>>>,
}

type MapOutcome = std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>;

impl PixelReport {
    /// Records the submission that holds the encoder the report was
    /// recorded into, so [`read`](Self::read) waits for it alone.
    pub fn submitted(&self, index: wgpu::SubmissionIndex) {
        if let Ok(mut s) = self.submission.lock() {
            *s = Some(index);
        }
    }

    /// Whether [`submitted`](Self::submitted) was called: until then the
    /// report is only recorded, and must not be read.
    pub fn is_submitted(&self) -> bool {
        self.submission.lock().is_ok_and(|s| s.is_some())
    }

    /// Samples that passed, summed over every counted draw since the clear.
    /// Blocks until the GPU has finished; the encoder the report was
    /// recorded into must have been submitted.
    pub fn read(&self, ctx: &GfxContext) -> Result<u64, String> {
        let mut v = self.value.lock().map_err(|_| "pixel count poisoned")?;
        if let Some(v) = *v {
            return Ok(v);
        }
        let rx = self.start_map()?;
        let submission_index = self.submission.lock().ok().and_then(|s| s.clone());
        ctx.device
            .poll(wgpu::PollType::Wait {
                submission_index,
                timeout: None,
            })
            .map_err(|e| format!("waiting for the pixel count: {e}"))?;
        let outcome = rx
            .lock()
            .map_err(|_| "pixel count poisoned")?
            .take()
            .ok_or("pixel-count readback dropped")?
            .recv()
            .map_err(|_| "pixel-count readback dropped".to_owned())?;
        let count = self.take_mapped(outcome)?;
        *v = Some(count);
        Ok(count)
    }

    /// The count if the GPU has already written it, without waiting:
    /// starts the read the first time, and answers `None` until the
    /// buffer is mapped. The value is the one [`read`](Self::read) gives.
    pub fn try_read(&self, ctx: &GfxContext) -> Result<Option<u64>, String> {
        let mut v = self.value.lock().map_err(|_| "pixel count poisoned")?;
        if let Some(v) = *v {
            return Ok(Some(v));
        }
        let rx = self.start_map()?;
        // Runs the callbacks of the work the GPU has finished.
        ctx.device
            .poll(wgpu::PollType::Poll)
            .map_err(|e| format!("polling for the pixel count: {e}"))?;
        let mut guard = rx.lock().map_err(|_| "pixel count poisoned")?;
        let outcome = match guard
            .as_ref()
            .ok_or("pixel-count readback dropped")?
            .try_recv()
        {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return Ok(None),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                return Err("pixel-count readback dropped".to_owned());
            }
        };
        guard.take();
        drop(guard);
        let count = self.take_mapped(outcome)?;
        *v = Some(count);
        Ok(Some(count))
    }

    /// Delivers an exclusively owned report through a callback after
    /// submission. Queue submissions drive the callback during play. A
    /// deterministic waiter may also poll the device for the submission.
    /// This report must have no other readers.
    pub fn on_ready(self, complete: impl FnOnce(Result<u64, String>) + Send + 'static) {
        let buffer = self.buffer.clone();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |mapped| {
            complete(self.take_mapped(mapped));
        });
    }

    /// Starts mapping the buffer once; the receiver of its outcome.
    fn start_map(&self) -> Result<&Arc<Mutex<Option<MapOutcome>>>, String> {
        let mut m = self.mapping.lock().map_err(|_| "pixel count poisoned")?;
        if m.is_none() {
            let (tx, rx) = std::sync::mpsc::channel();
            self.buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| {
                    let _ = tx.send(r);
                });
            *m = Some(rx);
        }
        Ok(&self.mapping)
    }

    fn take_mapped(&self, outcome: Result<(), wgpu::BufferAsyncError>) -> Result<u64, String> {
        outcome.map_err(|e| format!("mapping the pixel count: {e}"))?;
        let count = {
            let data = self
                .buffer
                .slice(..)
                .get_mapped_range()
                .map_err(|e| format!("pixel count: {e}"))?;
            u32::from_le_bytes(data[..4].try_into().expect("4 bytes")) as u64
        };
        self.buffer.unmap();
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
    /// Spacing of guest pixel centres in the current host count target.
    sample_stride: u32,
    total: wgpu::Buffer,
    single: (wgpu::BindGroupLayout, wgpu::ComputePipeline),
    multi: (wgpu::BindGroupLayout, wgpu::ComputePipeline),
    /// Count textures by `(width, height, samples)`.
    targets: HashMap<(u32, u32, u32), (wgpu::Texture, wgpu::TextureView)>,
    /// Scratch depth/stencil copies by `(width, height, samples, format)`.
    depths: HashMap<(u32, u32, u32, wgpu::TextureFormat), (wgpu::Texture, wgpu::TextureView)>,
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
                    min_binding_size: wgpu::BufferSize::new(20),
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
        let total = crate::churn::create_buffer(
            &ctx.device,
            &wgpu::BufferDescriptor {
                label: Some("gfx pixel count"),
                size: 4,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        );
        Self {
            total,
            sample_stride: 1,
            single: sum_pipeline(&ctx.device, false),
            multi: sum_pipeline(&ctx.device, true),
            targets: HashMap::new(),
            depths: HashMap::new(),
        }
    }

    /// Sum one coverage texel per guest pixel for a render-scaled pass.
    pub fn set_sample_stride(&mut self, stride: u32) {
        self.sample_stride = stride.max(1);
    }

    /// `CLEAR_REPORT_VALUE`: the count restarts at zero once `encoder`
    /// runs.
    pub fn clear(&mut self, encoder: &mut wgpu::CommandEncoder) {
        encoder.clear_buffer(&self.total, 0, None);
    }

    /// `GET_REPORT`: the samples counted since the last clear, as of this
    /// point in `encoder`. Read it after the encoder is submitted.
    pub fn report(&mut self, ctx: &GfxContext, encoder: &mut wgpu::CommandEncoder) -> PixelReport {
        let buffer = crate::churn::create_buffer(
            &ctx.device,
            &wgpu::BufferDescriptor {
                label: Some("gfx pixel count report"),
                size: 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            },
        );
        encoder.copy_buffer_to_buffer(&self.total, 0, &buffer, 0, 4);
        PixelReport {
            buffer: Arc::new(buffer),
            value: Arc::new(Mutex::new(None)),
            submission: Arc::new(Mutex::new(None)),
            mapping: Arc::new(Mutex::new(None)),
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

    /// Copies the depth/stencil texture `source` (all of it, every aspect)
    /// into a scratch texture of the same size, format and sample count,
    /// and returns the scratch view.
    pub(crate) fn depth_copy(
        &mut self,
        ctx: &GfxContext,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::Texture,
    ) -> &wgpu::TextureView {
        let size = source.size();
        let key = (
            size.width,
            size.height,
            source.sample_count(),
            source.format(),
        );
        let (scratch, _) = self.depths.entry(key).or_insert_with(|| {
            let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("gfx pixel count depth"),
                size,
                mip_level_count: 1,
                sample_count: key.2,
                dimension: wgpu::TextureDimension::D2,
                format: key.3,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            (texture, view)
        });
        encoder.copy_texture_to_texture(source.as_image_copy(), scratch.as_image_copy(), size);
        &self.depths[&key].1
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
        let stride = self.sample_stride;
        let x = rect[0].next_multiple_of(stride);
        let y = rect[1].next_multiple_of(stride);
        let w = (rect[0] + rect[2]).saturating_sub(x).div_ceil(stride);
        let h = (rect[1] + rect[3]).saturating_sub(y).div_ceil(stride);
        if w == 0 || h == 0 { return; }
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
                contents: &[x, y, w, h, stride]
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
        crate::churn::pass();
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("gfx pixel count sum"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
    }
}
