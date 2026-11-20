//! Render models on the GPU: each model's vertices, indices and inverse
//! rest frames uploaded once when it is added; per view one write of the
//! drawn objects' node matrices and instance records, and one render pass
//! into the game's own colour target and depth with an indexed draw per
//! (object, part), skinned in the vertex shader (`models.wgsl`).

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};

use crate::camera::{Mat4, RenderCamera};
use crate::cull::Frustum;
use crate::gpu::{DepthRule, Image, PassViewport};
use crate::instances::{FrameObjects, InstanceRecord, ObjectInput, pack};
use crate::model::{Model, ModelVertex, NodeMatrix};

const SHADER: &str = include_str!("models.wgsl");

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct FrameUniform {
    view_proj: Mat4,
    depth: [f32; 4],
    ambient: [f32; 4],
    light: [f32; 4],
}

/// The fixed lighting of the first cut (docs: "Shading"): ambient and one
/// directional light from above and to the side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lighting {
    pub ambient: [f32; 3],
    /// Towards the light, world space.
    pub direction: [f32; 3],
    pub diffuse: f32,
}

impl Lighting {
    pub const DEFAULT: Self = Self {
        ambient: [0.45, 0.45, 0.45],
        direction: [0.36, 0.48, 0.8],
        diffuse: 0.65,
    };
}

struct GpuModel {
    /// The model without its vertices and indices (draws, inverse frames).
    model: Model,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    group: wgpu::BindGroup,
    /// Per model-local material.
    materials: HashMap<u16, wgpu::BindGroup>,
}

fn render_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layouts: [&wgpu::BindGroupLayout; 3],
    color: wgpu::TextureFormat,
    depth: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("native models"),
        bind_group_layouts: &[Some(layouts[0]), Some(layouts[1]), Some(layouts[2])],
        immediate_size: 0,
    });
    let attributes = wgpu::vertex_attr_array![
        0 => Float32x3, 1 => Float32x2, 2 => Float32x3, 3 => Uint8x4, 4 => Float32x4
    ];
    gfx::churn::create_render_pipeline(device, &wgpu::RenderPipelineDescriptor {
        label: Some("native models"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<ModelVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &attributes,
            })],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            // S: the models' winding is not checked yet; draw both.
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
}

/// Every model uploaded so far and the current view's objects.
pub struct GpuModels {
    models: Vec<GpuModel>,
    frame: wgpu::Buffer,
    nodes: wgpu::Buffer,
    instances: wgpu::Buffer,
    frame_group: wgpu::BindGroup,
    frame_layout: wgpu::BindGroupLayout,
    model_layout: wgpu::BindGroupLayout,
    material_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    white: wgpu::TextureView,
    shader: wgpu::ShaderModule,
    pipelines: HashMap<(wgpu::TextureFormat, wgpu::TextureFormat), wgpu::RenderPipeline>,
    current: FrameObjects,
    pub lighting: Lighting,
}

fn texture(device: &wgpu::Device, queue: &wgpu::Queue, image: &Image) -> wgpu::TextureView {
    let size = wgpu::Extent3d {
        width: image.width.max(1),
        height: image.height.max(1),
        depth_or_array_layers: 1,
    };
    let t = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("native model base"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    gfx::churn::upload_bytes(image.rgba.len());
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

fn storage(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    gfx::churn::create_buffer(device, &wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(64),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn storage_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

impl GpuModels {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native models frame"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                storage_entry(1),
                storage_entry(2),
            ],
        });
        let model_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native models model"),
            entries: &[storage_entry(0)],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native models material"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let frame = gfx::churn::create_buffer(device, &wgpu::BufferDescriptor {
            label: Some("native models frame"),
            size: std::mem::size_of::<FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let nodes = storage(device, "native models nodes", 64 * 64 * 0x34);
        let instances = storage(device, "native models instances", 256 * 16);
        let frame_group = Self::frame_group(device, &frame_layout, &frame, &nodes, &instances);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("native models"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let white = texture(device, queue, &Image::solid([255; 4]));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native models"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        Self {
            models: Vec::new(),
            frame,
            nodes,
            instances,
            frame_group,
            frame_layout,
            model_layout,
            material_layout,
            sampler,
            white,
            shader,
            pipelines: HashMap::new(),
            current: FrameObjects::default(),
            lighting: Lighting::DEFAULT,
        }
    }

    fn frame_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        frame: &wgpu::Buffer,
        nodes: &wgpu::Buffer,
        instances: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        gfx::churn::create_bind_group(device, &wgpu::BindGroupDescriptor {
            label: Some("native models frame"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: frame.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: nodes.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: instances.as_entire_binding(),
                },
            ],
        })
    }

    /// Uploads `model` once; `base_map` gives a model-local material's base
    /// map (`None` draws white). Returns its index for [`ObjectInput::model`].
    pub fn add_model(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mut model: Model,
        base_map: &mut dyn FnMut(u16) -> Option<Image>,
    ) -> u32 {
        let init = |label, contents: &[u8], usage| {
            gfx::churn::create_buffer_init(device, &wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage,
            })
        };
        let mut verts = std::mem::take(&mut model.vertices);
        if verts.is_empty() {
            verts.push(ModelVertex::default());
        }
        let mut idx = std::mem::take(&mut model.indices);
        if idx.is_empty() {
            idx.push(0);
        }
        let mut inv = model.inverse_binds.clone();
        if inv.is_empty() {
            inv.push(NodeMatrix::IDENTITY);
        }
        let vertices = init("native model vertices", bytemuck::cast_slice(&verts), wgpu::BufferUsages::VERTEX);
        let indices = init("native model indices", bytemuck::cast_slice(&idx), wgpu::BufferUsages::INDEX);
        let inverses = init("native model inverses", bytemuck::cast_slice(&inv), wgpu::BufferUsages::STORAGE);
        let group = gfx::churn::create_bind_group(device, &wgpu::BindGroupDescriptor {
            label: Some("native model"),
            layout: &self.model_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: inverses.as_entire_binding(),
            }],
        });
        let mut materials = HashMap::new();
        for m in model.materials() {
            let view = base_map(m).map_or_else(|| self.white.clone(), |i| texture(device, queue, &i));
            materials.insert(m, self.material_group(device, &view));
        }
        self.models.push(GpuModel {
            model,
            vertices,
            indices,
            group,
            materials,
        });
        self.models.len() as u32 - 1
    }

    fn material_group(&self, device: &wgpu::Device, view: &wgpu::TextureView) -> wgpu::BindGroup {
        gfx::churn::create_bind_group(device, &wgpu::BindGroupDescriptor {
            label: Some("native model material"),
            layout: &self.material_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// The model added as `model` (draw tables, inverse frames).
    pub fn model(&self, model: u32) -> Option<&Model> {
        self.models.get(model as usize).map(|m| &m.model)
    }

    pub fn model_count(&self) -> usize {
        self.models.len()
    }

    /// The view: packs and culls `objects`, writes the uniform, the node
    /// matrices and the instance records (growing their buffers when
    /// needed). Returns what was packed (draws, culled, mismatched).
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        uploads: &mut gfx::ring::EncoderUploads<'_>,
        camera: &RenderCamera,
        depth: DepthRule,
        objects: &[ObjectInput<'_>],
    ) -> &FrameObjects {
        let view_proj = camera.view_projection();
        let frustum = Frustum::from_view_projection(&view_proj);
        let models = &self.models;
        self.current = pack(&|i: u32| models.get(i as usize).map(|m| &m.model), objects, Some(&frustum));
        let l = self.lighting;
        let u = FrameUniform {
            view_proj,
            depth: [depth.full_scale, 1.0 / camera.far, f32::from(u8::from(depth.floor)), 0.0],
            ambient: [l.ambient[0], l.ambient[1], l.ambient[2], 1.0],
            light: [l.direction[0], l.direction[1], l.direction[2], l.diffuse],
        };
        uploads.write(&self.frame, 0, bytemuck::bytes_of(&u));
        let node_bytes: &[u8] = bytemuck::cast_slice(&self.current.nodes);
        let inst_bytes: &[u8] = bytemuck::cast_slice(&self.current.instances);
        let mut regroup = false;
        if node_bytes.len() as u64 > self.nodes.size() {
            self.nodes = storage(device, "native models nodes", (node_bytes.len() as u64).next_power_of_two());
            regroup = true;
        }
        if inst_bytes.len() as u64 > self.instances.size() {
            self.instances = storage(device, "native models instances", (inst_bytes.len() as u64).next_power_of_two());
            regroup = true;
        }
        if regroup {
            self.frame_group = Self::frame_group(device, &self.frame_layout, &self.frame, &self.nodes, &self.instances);
        }
        if !node_bytes.is_empty() {
            uploads.write(&self.nodes, 0, node_bytes);
        }
        if !inst_bytes.is_empty() {
            uploads.write(&self.instances, 0, inst_bytes);
        }
        &self.current
    }

    fn pipeline(&mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat) {
        let (shader, layouts) = (&self.shader, [&self.frame_layout, &self.model_layout, &self.material_layout]);
        self.pipelines.entry((color, depth)).or_insert_with(|| {
            render_pipeline(device, shader, layouts, color, depth)
        });
    }

    /// Records the pass for the last [`prepare`](Self::prepare): loads and
    /// keeps the target and depth, one indexed draw per (object, part).
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
    ) -> usize {
        if self.current.draws.is_empty() {
            return 0;
        }
        self.pipeline(device, color_format, depth_format);
        let pipeline = &self.pipelines[&(color_format, depth_format)];
        let keep = wgpu::Operations {
            load: wgpu::LoadOp::Load,
            store: wgpu::StoreOp::Store,
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native models"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: keep,
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: depth_format.has_stencil_aspect().then_some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_viewport(viewport.x, viewport.y, viewport.width, viewport.height, 0.0, 1.0);
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &self.frame_group, &[]);
        let mut bound: Option<(u32, Option<u16>)> = None;
        let mut recorded = 0;
        for d in &self.current.draws {
            let Some(m) = self.models.get(d.model as usize) else { continue };
            let Some(group) = m.materials.get(&d.material) else { continue };
            if bound.map(|b| b.0) != Some(d.model) {
                pass.set_vertex_buffer(0, m.vertices.slice(..));
                pass.set_index_buffer(m.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.set_bind_group(1, &m.group, &[]);
                bound = Some((d.model, None));
            }
            if bound.and_then(|b| b.1) != Some(d.material) {
                pass.set_bind_group(2, group, &[]);
                bound = Some((d.model, Some(d.material)));
            }
            pass.draw_indexed(d.first_index..d.first_index + d.index_count, 0, d.instance..d.instance + 1);
            recorded += 1;
        }
        recorded
    }
}

/// The record layout the shader reads (checked against `models.wgsl`).
pub const INSTANCE_RECORD_SIZE: usize = std::mem::size_of::<InstanceRecord>();
