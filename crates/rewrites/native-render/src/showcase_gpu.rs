//! A model shown on its own on the GPU, as a menu picture
//! (docs/game/skulls.md, "The model picture"): the model's opaque parts at
//! rest, uploaded once, drawn into a target of its own with the lighting,
//! tint, decal and silhouette of `showcase.wgsl`, then composited into the
//! menu's colour target at a rectangle. The placement and look are
//! [`crate::showcase`]'s.

use wgpu::util::DeviceExt;

use crate::gpu::Image;
use crate::model::{Model, ModelVertex};
use crate::showcase::{Look, Orientation, Placement, Uniform};
use crate::world::Aabb;

const SHADER: &str = include_str!("showcase.wgsl");
const COMPOSITE: &str = include_str!("showcase_composite.wgsl");
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// The picture's own colour format (premultiplied).
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// One run of a [`ShowcaseMesh`]'s indices drawn with one material's base
/// map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShowcasePart {
    /// The model-local material.
    pub material: u16,
    pub first_index: u32,
    pub index_count: u32,
}

/// A model's opaque parts at rest: its vertices as stored (rest space),
/// the opaque draws' indices, each draw's material run, and its bounds.
#[derive(Clone, Debug)]
pub struct ShowcaseMesh {
    pub vertices: Vec<ModelVertex>,
    pub indices: Vec<u32>,
    pub parts: Vec<ShowcasePart>,
    pub bounds: Aabb,
}

impl ShowcaseMesh {
    /// `model`'s opaque draws, a part per draw. `None` with nothing opaque
    /// or no bounds.
    pub fn from_model(model: &Model) -> Option<Self> {
        let mut indices = Vec::new();
        let mut parts = Vec::new();
        for d in &model.draws {
            let range = d.first_index as usize..(d.first_index + d.index_count) as usize;
            let first_index = indices.len() as u32;
            indices.extend_from_slice(model.indices.get(range)?);
            parts.push(ShowcasePart {
                material: d.material,
                first_index,
                index_count: d.index_count,
            });
        }
        let mut bounds = Aabb::EMPTY;
        for &i in &indices {
            bounds.add(model.vertices.get(i as usize)?.position);
        }
        (!indices.is_empty() && !bounds.is_empty()).then(|| Self {
            vertices: model.vertices.clone(),
            indices,
            parts,
            bounds,
        })
    }

    /// The mesh turned by `orientation` into the picture's space (x
    /// forward, y left, z up): positions and normals rotated, bounds
    /// recomputed from the drawn vertices, so the framing, the decal and
    /// the sway all follow.
    pub fn oriented(&self, orientation: &Orientation) -> Self {
        let vertices: Vec<ModelVertex> = self
            .vertices
            .iter()
            .map(|v| ModelVertex {
                position: orientation.apply(v.position),
                normal: orientation.apply(v.normal),
                ..*v
            })
            .collect();
        let mut bounds = Aabb::EMPTY;
        for &i in &self.indices {
            if let Some(v) = vertices.get(i as usize) {
                bounds.add(v.position);
            }
        }
        Self {
            vertices,
            indices: self.indices.clone(),
            parts: self.parts.clone(),
            bounds,
        }
    }
}

struct Part {
    first_index: u32,
    index_count: u32,
    /// Its material's base map, or white (the bone tint alone).
    base: wgpu::TextureView,
}

struct Mesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    parts: Vec<Part>,
    placement: Placement,
}

struct Target {
    color: wgpu::Texture,
    view: wgpu::TextureView,
    depth: wgpu::TextureView,
}

/// The picture's renderer: one model, one glyph, one target.
pub struct Showcase {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// White: a part with no base map of its own shows the tint alone.
    white: wgpu::TextureView,
    glyph: wgpu::TextureView,
    mesh: Option<Mesh>,
    target: Option<Target>,
    composite_layout: wgpu::BindGroupLayout,
    composite_module: wgpu::ShaderModule,
    composites: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
}

fn texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    (width, height): (u32, u32),
    format: wgpu::TextureFormat,
    bytes: &[u8],
) -> wgpu::TextureView {
    device
        .create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            bytes,
        )
        .create_view(&Default::default())
}

fn entry(binding: u32, ty: wgpu::BindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty,
        count: None,
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    entry(
        binding,
        wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
    )
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    entry(
        binding,
        wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
    )
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    entry(
        binding,
        wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
    )
}

impl Showcase {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("showcase"),
            entries: &[
                uniform_entry(0),
                texture_entry(1),
                sampler_entry(2),
                texture_entry(3),
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("showcase"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("showcase"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x3];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("showcase"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
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
                // The models' winding is not checked (crate::model_gpu).
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("showcase"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("showcase composite"),
            entries: &[uniform_entry(0), texture_entry(1), sampler_entry(2)],
        });
        let composite_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("showcase composite"),
            source: wgpu::ShaderSource::Wgsl(COMPOSITE.into()),
        });
        Self {
            pipeline,
            layout,
            sampler,
            white: texture(device, queue, "showcase white", (1, 1), FORMAT, &[255; 4]),
            glyph: texture(
                device,
                queue,
                "showcase glyph",
                (1, 1),
                wgpu::TextureFormat::R8Unorm,
                &[0],
            ),
            mesh: None,
            target: None,
            composite_layout,
            composite_module,
            composites: Vec::new(),
        }
    }

    /// Uploads the model to show and its materials' base maps: each part
    /// draws with the image `bases` gives for its material, and a part
    /// whose material has none (or a malformed one) draws white, so it
    /// takes the look's tint alone.
    pub fn set_mesh(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mesh: &ShowcaseMesh,
        bases: &[(u16, Image)],
    ) {
        let Some(placement) = Placement::of(&mesh.bounds) else {
            self.mesh = None;
            return;
        };
        let init = |label, contents: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage,
            })
        };
        let mut uploaded: Vec<(u16, wgpu::TextureView)> = Vec::new();
        for (material, image) in bases {
            let ok = image.width > 0
                && image.height > 0
                && image.rgba.len() == (4 * image.width * image.height) as usize;
            if ok && !uploaded.iter().any(|(m, _)| m == material) {
                let view = texture(
                    device,
                    queue,
                    "showcase base",
                    (image.width, image.height),
                    FORMAT,
                    &image.rgba,
                );
                uploaded.push((*material, view));
            }
        }
        let count = mesh.indices.len() as u32;
        let parts = mesh
            .parts
            .iter()
            .filter(|p| p.index_count > 0 && p.first_index.saturating_add(p.index_count) <= count)
            .map(|p| Part {
                first_index: p.first_index,
                index_count: p.index_count,
                base: uploaded
                    .iter()
                    .find(|(m, _)| *m == p.material)
                    .map_or_else(|| self.white.clone(), |(_, v)| v.clone()),
            })
            .collect();
        self.mesh = Some(Mesh {
            vertices: init(
                "showcase vertices",
                bytemuck::cast_slice(&mesh.vertices),
                wgpu::BufferUsages::VERTEX,
            ),
            indices: init(
                "showcase indices",
                bytemuck::cast_slice(&mesh.indices),
                wgpu::BufferUsages::INDEX,
            ),
            parts,
            placement,
        });
    }

    /// The uploaded model's placement.
    pub fn placement(&self) -> Option<Placement> {
        self.mesh.as_ref().map(|m| m.placement)
    }

    /// The decal: a `size` by `size` coverage mask, a byte a texel, row 0
    /// at the top. A mask of the wrong length clears the decal.
    pub fn set_glyph(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: u32,
        coverage: &[u8],
    ) {
        let ok = size > 0 && coverage.len() == (size * size) as usize;
        self.glyph = if ok {
            texture(
                device,
                queue,
                "showcase glyph",
                (size, size),
                wgpu::TextureFormat::R8Unorm,
                coverage,
            )
        } else {
            texture(
                device,
                queue,
                "showcase glyph",
                (1, 1),
                wgpu::TextureFormat::R8Unorm,
                &[0],
            )
        };
    }

    fn target(&mut self, device: &wgpu::Device, width: u32, height: u32) -> &Target {
        let fits = self
            .target
            .as_ref()
            .is_some_and(|t| t.color.width() == width && t.color.height() == height);
        if !fits {
            let make = |label, format, usage| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
            };
            let color = make(
                "showcase picture",
                FORMAT,
                wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
            );
            let depth = make(
                "showcase depth",
                DEPTH,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            );
            self.target = Some(Target {
                view: color.create_view(&Default::default()),
                depth: depth.create_view(&Default::default()),
                color,
            });
        }
        self.target.as_ref().expect("made above")
    }

    /// Records the picture: the target cleared to transparent, the model
    /// drawn as `look` says. False, recording nothing, with no model.
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        width: u32,
        height: u32,
        look: &Look,
    ) -> bool {
        let Some(placement) = self.placement() else {
            return false;
        };
        if width == 0 || height == 0 {
            return false;
        }
        let uniform = Uniform::new(&placement, look, width, height);
        // A buffer per draw: two copies of the target recorded in one
        // submission each keep their own values.
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("showcase frame"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let mesh = self.mesh.as_ref().expect("placement implies a mesh");
        let groups: Vec<(u32, u32, wgpu::BindGroup)> = mesh
            .parts
            .iter()
            .map(|part| {
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("showcase"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&part.base),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(&self.glyph),
                        },
                    ],
                });
                (part.first_index, part.index_count, group)
            })
            .collect();
        let (vertices, indices) = (mesh.vertices.clone(), mesh.indices.clone());
        let target = self.target(device, width, height);
        let (view, depth) = (target.view.clone(), target.depth.clone());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("showcase"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        for (first, count, group) in &groups {
            pass.set_bind_group(0, group, &[]);
            pass.draw_indexed(*first..first + count, 0, 0..1);
        }
        true
    }

    /// The last picture drawn (for a readback).
    pub fn picture(&self) -> Option<&wgpu::Texture> {
        self.target.as_ref().map(|t| &t.color)
    }

    fn composite_pipeline(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> wgpu::RenderPipeline {
        if let Some((_, p)) = self.composites.iter().find(|(f, _)| *f == format) {
            return p.clone();
        }
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("showcase composite"),
            bind_group_layouts: &[Some(&self.composite_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("showcase composite"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &self.composite_module,
                entry_point: Some("vs_quad"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &self.composite_module,
                entry_point: Some("fs_quad"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        self.composites.push((format, pipeline.clone()));
        pipeline
    }

    /// Records the last picture drawn into `target` (format `format`) at
    /// `rect` (`x, y, width, height` in its texels), blended over what is
    /// there and scaled by `alpha`. False with no picture or an empty rect.
    pub fn composite(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        rect: [f32; 4],
        alpha: f32,
    ) -> bool {
        let Some(picture) = self.target.as_ref().map(|t| t.view.clone()) else {
            return false;
        };
        let [x, y, w, h] = rect;
        if !(w >= 1.0 && h >= 1.0)
            || !x.is_finite()
            || !y.is_finite()
            || !alpha.is_finite()
            || alpha <= 0.0
        {
            return false;
        }
        let size = target.texture().size();
        let pipeline = self.composite_pipeline(device, format);
        let words: [f32; 12] = [
            alpha.min(1.0),
            0.0,
            0.0,
            0.0,
            x,
            y,
            w,
            h,
            size.width as f32,
            size.height as f32,
            0.0,
            0.0,
        ];
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("showcase composite"),
            contents: bytemuck::cast_slice(&words),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("showcase composite"),
            layout: &self.composite_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&picture),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("showcase composite"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
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
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..4, 0..1);
        true
    }
}
