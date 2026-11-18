//! The world on the GPU: buffers, textures and bind groups made once at
//! load; per frame one uniform write, the indirect slots' instance counts
//! and one render pass with a `multi_draw_indexed_indirect` per batch,
//! into the game's own colour target and depth.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::camera::{Mat4, RenderCamera};
use crate::cull::{CullStats, Frustum, IndirectSlot, SLOT_SIZE, cull, indirect_slots};
use crate::world::{LightmapKey, World, WorldVertex};

/// An RGBA8 image (linear rows, no padding).
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn solid(rgba: [u8; 4]) -> Self {
        Self {
            width: 1,
            height: 1,
            rgba: rgba.to_vec(),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct FrameUniform {
    view_proj: Mat4,
    depth: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct BatchUniform {
    flags: [f32; 4],
}

/// The depth the pass writes: the format's full scale and whether to floor
/// to its step (the game's W-buffer; [`crate::camera::w_depth`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DepthRule {
    pub full_scale: f32,
    pub floor: bool,
}

impl DepthRule {
    pub const D24: Self = Self {
        full_scale: crate::camera::D24_SCALE,
        floor: true,
    };
}

/// Where the pass draws, in target pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PassViewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

pub struct GpuWorld {
    world: World,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    indirect: wgpu::Buffer,
    slots: Vec<IndirectSlot>,
    frame: wgpu::Buffer,
    frame_group: wgpu::BindGroup,
    batch_groups: Vec<wgpu::BindGroup>,
    frame_layout: wgpu::BindGroupLayout,
    batch_layout: wgpu::BindGroupLayout,
    shader: wgpu::ShaderModule,
    pipelines: HashMap<(wgpu::TextureFormat, wgpu::TextureFormat), wgpu::RenderPipeline>,
}

const SHADER: &str = include_str!("world.wgsl");

fn texture(device: &wgpu::Device, queue: &wgpu::Queue, image: &Image, label: &str) -> wgpu::TextureView {
    let size = wgpu::Extent3d {
        width: image.width.max(1),
        height: image.height.max(1),
        depth_or_array_layers: 1,
    };
    let t = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        t.as_image_copy(),
        &image.rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * size.width),
            rows_per_image: Some(size.height),
        },
        size,
    );
    t.create_view(&wgpu::TextureViewDescriptor::default())
}

impl GpuWorld {
    /// Uploads `world` once. `base_map` gives a material's base map and
    /// `lightmap` a page through its palette; `None` draws white (the
    /// per-material fallback to the D3D path is decided by the caller,
    /// which leaves such materials out of the world).
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world: World,
        base_map: &mut dyn FnMut(u16) -> Option<Image>,
        lightmap: &mut dyn FnMut(LightmapKey) -> Option<Image>,
    ) -> Self {
        let init = |label, contents: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage,
            })
        };
        // An empty buffer cannot be bound: keep one element.
        let mut verts = world.vertices.clone();
        if verts.is_empty() {
            verts.push(WorldVertex::default());
        }
        let mut idx = world.indices.clone();
        if idx.is_empty() {
            idx.push(0);
        }
        let mut slots = indirect_slots(&world);
        let slot_bytes: Vec<IndirectSlot> = if slots.is_empty() {
            vec![IndirectSlot::default()]
        } else {
            slots.clone()
        };
        let vertices = init("native world vertices", bytemuck::cast_slice(&verts), wgpu::BufferUsages::VERTEX);
        let indices = init("native world indices", bytemuck::cast_slice(&idx), wgpu::BufferUsages::INDEX);
        let indirect = init(
            "native world indirect",
            bytemuck::cast_slice(&slot_bytes),
            wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
        );
        let frame = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("native world frame"),
            size: std::mem::size_of::<FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_entry = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native world frame"),
            entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT)],
        });
        let batch_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native world batch"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::FRAGMENT),
                tex_entry(1),
                tex_entry(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let frame_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native world frame"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: frame.as_entire_binding(),
            }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("native world"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let white = texture(device, queue, &Image::solid([255; 4]), "native world white");
        let mut bases: HashMap<u16, wgpu::TextureView> = HashMap::new();
        let mut pages: HashMap<LightmapKey, wgpu::TextureView> = HashMap::new();
        let mut batch_groups = Vec::with_capacity(world.batches.len());
        for b in &world.batches {
            let base = bases
                .entry(b.material)
                .or_insert_with(|| {
                    base_map(b.material).map_or_else(|| white.clone(), |i| texture(device, queue, &i, "native world base"))
                })
                .clone();
            let lit = b.lightmap.and_then(|k| {
                if let Some(v) = pages.get(&k) {
                    return Some(v.clone());
                }
                let v = texture(device, queue, &lightmap(k)?, "native world lightmap");
                pages.insert(k, v.clone());
                Some(v)
            });
            let uniform = init(
                "native world batch",
                bytemuck::bytes_of(&BatchUniform {
                    flags: [f32::from(u8::from(lit.is_some())), 0.0, 0.0, 0.0],
                }),
                wgpu::BufferUsages::UNIFORM,
            );
            let page = lit.unwrap_or_else(|| white.clone());
            batch_groups.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("native world batch"),
                layout: &batch_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&base),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&page),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            }));
        }
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native world"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        slots.shrink_to_fit();
        Self {
            world,
            vertices,
            indices,
            indirect,
            slots,
            frame,
            frame_group,
            batch_groups,
            frame_layout,
            batch_layout,
            shader,
            pipelines: HashMap::new(),
        }
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    fn pipeline(&mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        let (shader, fl, bl) = (&self.shader, &self.frame_layout, &self.batch_layout);
        self.pipelines.entry((color, depth)).or_insert_with(|| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("native world"),
                bind_group_layouts: &[Some(fl), Some(bl)],
                immediate_size: 0,
            });
            let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x2, 3 => Float32x3];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("native world"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<WorldVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes,
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    // S: the BSP's winding is not checked yet; draw both.
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: depth,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: color,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        })
    }

    /// The frame's camera: culls on the CPU and writes the uniform and the
    /// slots' instance counts.
    pub fn prepare(&mut self, queue: &wgpu::Queue, camera: &RenderCamera, depth: DepthRule) -> CullStats {
        let view_proj = camera.view_projection();
        let stats = cull(&self.world, &Frustum::from_view_projection(&view_proj), &mut self.slots);
        let u = FrameUniform {
            view_proj,
            depth: [depth.full_scale, 1.0 / camera.far, f32::from(u8::from(depth.floor)), 0.0],
        };
        queue.write_buffer(&self.frame, 0, bytemuck::bytes_of(&u));
        if !self.slots.is_empty() {
            queue.write_buffer(&self.indirect, 0, bytemuck::cast_slice(&self.slots));
        }
        stats
    }

    /// Records the pass: loads and keeps the target and depth, draws every
    /// batch with its slot range.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        color_format: wgpu::TextureFormat,
        depth: &wgpu::TextureView,
        depth_format: wgpu::TextureFormat,
        viewport: PassViewport,
    ) {
        self.pipeline(device, color_format, depth_format);
        let pipeline = &self.pipelines[&(color_format, depth_format)];
        let has_stencil = depth_format.has_stencil_aspect();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native world"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: has_stencil.then_some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if self.slots.is_empty() {
            return;
        }
        pass.set_viewport(viewport.x, viewport.y, viewport.width, viewport.height, 0.0, 1.0);
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &self.frame_group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        for (b, group) in self.world.batches.iter().zip(&self.batch_groups) {
            pass.set_bind_group(1, group, &[]);
            pass.multi_draw_indexed_indirect(&self.indirect, u64::from(b.first_draw) * SLOT_SIZE, b.draw_count);
        }
    }
}
