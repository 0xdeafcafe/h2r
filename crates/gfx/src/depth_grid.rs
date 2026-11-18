//! The occlusion test's depth grid, made on the GPU
//! (docs/graphics/occlusion-culling.md).
//!
//! [`DepthGrid`] reduces a rectangle of a depth texture (the main view's
//! depth after its opaque layers) to a small grid, `64 x 32` by default,
//! each cell holding the largest raw depth of the pixels whose centre falls
//! in it. Pixel `p` of a `src`-wide rectangle belongs to cell
//! `floor((p + 0.5) · cells / src)`, the rule
//! `engine::render_loop::occlusion::DepthGrid::reduce` uses on the CPU. A
//! cell no pixel centre falls in reads `1.0` (far). Raw depth grows with
//! view depth under both the W-buffer and a projective z, so the largest
//! raw value is the farthest; the CPU maps it to view depth.
//!
//! The grid comes back to the CPU through [`PendingGrid`] without a GPU
//! wait: the copy is mapped asynchronously and [`PendingGrid::try_take`]
//! answers `None` until the map has completed. The occlusion tracker uses
//! a grid one or two frames old.
//!
//! Additive: nothing calls this from the draw path yet.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::context::GfxContext;

const SHADER: &str = r#"
@group(0) @binding(0) var depth_tex: texture_depth_2d;
@group(0) @binding(1) var<storage, read_write> out_cells: array<f32>;
// The rectangle (x, y, width, height) in texels, then the grid size.
struct Params { rx: u32, ry: u32, rw: u32, rh: u32, gw: u32, gh: u32, pad0: u32, pad1: u32 }
@group(0) @binding(2) var<uniform> params: Params;

// The first pixel whose centre is in cell c: the smallest p with
// (2p + 1) * cells >= 2 c src.
fn first(c: u32, src: u32, cells: u32) -> u32 {
    let a = i32(2u * c * src) - i32(cells);
    if (a <= 0) {
        return 0u;
    }
    let b = 2u * cells;
    return (u32(a) + b - 1u) / b;
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.gw || id.y >= params.gh) {
        return;
    }
    let x0 = first(id.x, params.rw, params.gw);
    let x1 = min(first(id.x + 1u, params.rw, params.gw), params.rw);
    let y0 = first(id.y, params.rh, params.gh);
    let y1 = min(first(id.y + 1u, params.rh, params.gh), params.rh);
    var m = -1.0;
    for (var y = y0; y < y1; y = y + 1u) {
        for (var x = x0; x < x1; x = x + 1u) {
            let d = textureLoad(depth_tex, vec2<i32>(i32(params.rx + x), i32(params.ry + y)), 0);
            m = max(m, d);
        }
    }
    if (m < 0.0) {
        m = 1.0;
    }
    out_cells[id.y * params.gw + id.x] = m;
}
"#;

/// The compute pipeline (made once per device).
pub struct DepthGrid {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

/// A rectangle of a texture, in texels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

const PENDING: u8 = 0;
const READY: u8 = 1;
const FAILED: u8 = 2;

/// A grid on its way back to the CPU.
pub struct PendingGrid {
    pub width: u32,
    pub height: u32,
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
    mapping: bool,
}

impl PendingGrid {
    /// Asks for the map (call after the encoder holding the reduction is
    /// submitted). Idempotent.
    pub fn map(&mut self) {
        if self.mapping {
            return;
        }
        self.mapping = true;
        let state = Arc::clone(&self.state);
        self.buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| {
                state.store(if r.is_ok() { READY } else { FAILED }, Ordering::Release);
            });
    }

    /// The grid's raw depths (row 0 the top) once the map completed, else
    /// `None`. Never waits; something else must poll the device (the
    /// renderer's own `poll` at `Swap` does).
    pub fn try_take(&self) -> Option<Vec<f32>> {
        if self.state.load(Ordering::Acquire) != READY {
            return None;
        }
        let data = self.buffer.slice(..).get_mapped_range().ok()?;
        let out = data
            .as_chunks::<4>()
            .0
            .iter()
            .take((self.width * self.height) as usize)
            .map(|c| f32::from_le_bytes(*c))
            .collect();
        drop(data);
        self.buffer.unmap();
        Some(out)
    }

    /// The map failed (device lost): give up on this grid.
    pub fn failed(&self) -> bool {
        self.state.load(Ordering::Acquire) == FAILED
    }
}

impl DepthGrid {
    pub fn new(ctx: &GfxContext) -> Self {
        let device = &ctx.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gfx depth grid"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty,
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gfx depth grid"),
            entries: &[
                entry(
                    0,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(
                    1,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                entry(
                    2,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gfx depth grid"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("gfx depth grid"),
            layout: Some(&pl),
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self { pipeline, layout }
    }

    /// Records into `encoder` the reduction of `rect` of `depth` (a
    /// single-sample depth texture with `TEXTURE_BINDING`) to a `width x
    /// height` grid, and its copy to a mappable buffer. Submit `encoder`,
    /// then [`PendingGrid::map`].
    pub fn reduce(
        &self,
        ctx: &GfxContext,
        encoder: &mut wgpu::CommandEncoder,
        depth: &wgpu::Texture,
        rect: Rect,
        width: u32,
        height: u32,
    ) -> PendingGrid {
        let device = &ctx.device;
        let (width, height) = (width.max(1), height.max(1));
        let size = (width * height * 4) as u64;
        let cells = crate::churn::create_buffer(
            device,
            &wgpu::BufferDescriptor {
                label: Some("gfx depth grid cells"),
                size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            },
        );
        let read = crate::churn::create_buffer(
            device,
            &wgpu::BufferDescriptor {
                label: Some("gfx depth grid readback"),
                size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            },
        );
        let params: [u32; 8] = [rect.x, rect.y, rect.width, rect.height, width, height, 0, 0];
        let uniform = crate::churn::create_buffer(
            device,
            &wgpu::BufferDescriptor {
                label: Some("gfx depth grid params"),
                size: 32,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        );
        let bytes: Vec<u8> = params.iter().flat_map(|v| v.to_le_bytes()).collect();
        crate::churn::queue_write();
        ctx.queue.write_buffer(&uniform, 0, &bytes);
        let view = depth.create_view(&wgpu::TextureViewDescriptor {
            aspect: wgpu::TextureAspect::DepthOnly,
            ..Default::default()
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gfx depth grid"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: cells.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        {
            crate::churn::side_pass();
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("gfx depth grid"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
        }
        encoder.copy_buffer_to_buffer(&cells, 0, &read, 0, size);
        PendingGrid {
            width,
            height,
            buffer: read,
            state: Arc::new(AtomicU8::new(PENDING)),
            mapping: false,
        }
    }
}
