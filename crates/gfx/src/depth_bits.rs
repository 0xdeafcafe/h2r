//! A `D24S8` depth/stencil surface's raw bits as an RGBA8 texture, made on
//! the GPU.
//!
//! A depth-format texture is sampled as its raw bits
//! ([`convert::depth_channels`](crate::convert::depth_channels): red, green
//! and blue the depth bytes from most to least significant, alpha the
//! stencil). For a render-to-texture depth level (a shadow map) that used
//! to mean a readback ([`SurfaceStore::read_depth_as`]), the guest words
//! `z << 8 | stencil` with `z = round(clamp(d, 0, 1) * 16777215)`, a CPU
//! decode and an upload. [`DepthBits`] computes the same texels with a
//! compute pass that reads the surface's depth and stencil aspects, so
//! nothing waits for the GPU.
//!
//! The rounding is the CPU's exactly: `f32` multiply (IEEE, as on the CPU),
//! then halves away from zero (`f32::round`), written as `trunc` plus a
//! comparison of the exact remainder rather than WGSL's `round`, which
//! rounds halves to even. `depth_bits_*` tests compare the two paths over
//! every 24-bit depth value and a sweep of other `f32` depths.
//!
//! [`SurfaceStore::read_depth_as`]: crate::surface::SurfaceStore::read_depth_as

use crate::context::GfxContext;

const SHADER: &str = r#"
@group(0) @binding(0) var depth_tex: texture_depth_2d;
@group(0) @binding(1) var stencil_tex: texture_2d<u32>;
@group(0) @binding(2) var<storage, read_write> out_words: array<u32>;
struct Params { width: u32, height: u32, row_words: u32, has_stencil: u32 }
@group(0) @binding(3) var<uniform> params: Params;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.width || id.y >= params.height) {
        return;
    }
    let p = vec2<i32>(id.xy);
    let x = clamp(textureLoad(depth_tex, p, 0), 0.0, 1.0) * 16777215.0;
    let t = trunc(x);
    var z = u32(t);
    if (x - t >= 0.5) {
        z = z + 1u;
    }
    var s = 0u;
    if (params.has_stencil != 0u) {
        s = textureLoad(stencil_tex, p, 0).r & 0xffu;
    }
    out_words[id.y * params.row_words + id.x] =
        ((z >> 16u) & 0xffu) | (((z >> 8u) & 0xffu) << 8u) | ((z & 0xffu) << 16u) | (s << 24u);
}
"#;

/// The compute pipeline (made once per device).
pub struct DepthBits {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

impl DepthBits {
    /// Builds the pipeline.
    pub fn new(ctx: &GfxContext) -> Self {
        let device = &ctx.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gfx depth bits"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty,
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gfx depth bits"),
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
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(
                    2,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                entry(
                    3,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gfx depth bits"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("gfx depth bits"),
            layout: Some(&pl),
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self { pipeline, layout }
    }

    /// Records into `encoder` the conversion of `depth` (a single-sample
    /// `Depth32FloatStencil8` or `Depth32Float` texture of `size`, with
    /// `TEXTURE_BINDING`) into a new `Rgba8Unorm` texture of the same size
    /// holding [`depth_channels`](crate::convert::depth_channels) of its
    /// `D24S8` words. The texture is valid once `encoder` is submitted.
    pub fn convert(
        &self,
        ctx: &GfxContext,
        encoder: &mut wgpu::CommandEncoder,
        depth: &wgpu::Texture,
        size: (u32, u32),
    ) -> wgpu::Texture {
        let device = &ctx.device;
        let (w, h) = (size.0.max(1), size.1.max(1));
        let row_bytes = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let words = crate::churn::create_buffer(
            device,
            &wgpu::BufferDescriptor {
                label: Some("gfx depth bits words"),
                size: (row_bytes * h) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            },
        );
        let has_stencil = depth.format().has_stencil_aspect();
        let params: [u32; 4] = [w, h, row_bytes / 4, has_stencil as u32];
        let uniform = crate::churn::create_buffer(
            device,
            &wgpu::BufferDescriptor {
                label: Some("gfx depth bits params"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        );
        let bytes: Vec<u8> = params.iter().flat_map(|v| v.to_le_bytes()).collect();
        crate::churn::queue_write();
        ctx.queue.write_buffer(&uniform, 0, &bytes);
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor {
            aspect: wgpu::TextureAspect::DepthOnly,
            ..Default::default()
        });
        // Without a stencil aspect the binding still needs a `Uint` view;
        // a 1x1 dummy stands in and is never read.
        let dummy;
        let stencil_view = if has_stencil {
            depth.create_view(&wgpu::TextureViewDescriptor {
                aspect: wgpu::TextureAspect::StencilOnly,
                ..Default::default()
            })
        } else {
            dummy = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("gfx depth bits no stencil"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Uint,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            dummy.create_view(&Default::default())
        };
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gfx depth bits"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&stencil_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: words.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        {
            crate::churn::pass();
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("gfx depth bits"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
        }
        let out = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gfx depth bits"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        encoder.copy_buffer_to_texture(
            wgpu::TexelCopyBufferInfo {
                buffer: &words,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row_bytes),
                    rows_per_image: Some(h),
                },
            },
            wgpu::TexelCopyTextureInfo {
                texture: &out,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        out
    }
}
