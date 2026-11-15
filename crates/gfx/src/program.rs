//! Resources for translated Xbox shader programs.
//!
//! `crates/shaders` emits one WGSL module per vertex/pixel pair with a fixed
//! resource contract (its `docs/shaders.md`):
//!
//! | group | binding | contents |
//! |---|---|---|
//! | 0 | 0 | vertex uniforms (vertex stage) |
//! | 0 | 1 | pixel uniforms (fragment stage) |
//! | 1 | 2n | texture for stage n (2D, 3D or cube) |
//! | 1 | 2n+1 | sampler for stage n |
//!
//! [`ProgramBindings`] builds the layouts for that contract, wraps a module
//! into a [`ShaderProgram`] for [`crate::Renderer::register_shader`], and
//! makes the per-draw bind groups: uniform buffers, the stage textures (a
//! 1x1 stand-in, opaque black, for a stage the program samples but nothing
//! usable is bound to) and samplers cached by [`SamplerKey`].

use std::collections::HashMap;

use wgpu::util::DeviceExt;

use crate::pipeline::ShaderProgram;
use crate::translate::SamplerKey;

/// Texture dimension per stage a program samples (`None`: not sampled).
pub type StageDims = [Option<wgpu::TextureViewDimension>; 4];

/// Layouts, samplers and stand-in textures for translated programs.
pub struct ProgramBindings {
    uniforms: wgpu::BindGroupLayout,
    layouts: HashMap<StageDims, (Option<wgpu::BindGroupLayout>, wgpu::PipelineLayout)>,
    samplers: HashMap<SamplerKey, wgpu::Sampler>,
    dummies: HashMap<wgpu::TextureViewDimension, wgpu::TextureView>,
}

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// Whether a view of `format` can be bound where a program expects a
/// filterable float texture.
pub fn filterable(format: wgpu::TextureFormat, features: wgpu::Features) -> bool {
    matches!(
        format.sample_type(None, Some(features)),
        Some(wgpu::TextureSampleType::Float { filterable: true })
    )
}

impl ProgramBindings {
    /// Creates the shared uniform layout.
    pub fn new(device: &wgpu::Device) -> Self {
        let uniforms = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("program uniforms"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::VERTEX),
                uniform_entry(1, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        Self {
            uniforms,
            layouts: HashMap::new(),
            samplers: HashMap::new(),
            dummies: HashMap::new(),
        }
    }

    fn layout(
        &mut self,
        device: &wgpu::Device,
        dims: StageDims,
    ) -> &(Option<wgpu::BindGroupLayout>, wgpu::PipelineLayout) {
        let uniforms = &self.uniforms;
        self.layouts.entry(dims).or_insert_with(|| {
            let mut entries = Vec::new();
            for (n, d) in dims.iter().enumerate() {
                let Some(d) = d else { continue };
                entries.push(wgpu::BindGroupLayoutEntry {
                    binding: 2 * n as u32,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: *d,
                        multisampled: false,
                    },
                    count: None,
                });
                entries.push(wgpu::BindGroupLayoutEntry {
                    binding: 2 * n as u32 + 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                });
            }
            let textures = (!entries.is_empty()).then(|| {
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("program textures"),
                    entries: &entries,
                })
            });
            let mut groups = vec![Some(uniforms)];
            if let Some(t) = &textures {
                groups.push(Some(t));
            }
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("program layout"),
                bind_group_layouts: &groups,
                immediate_size: 0,
            });
            (textures, layout)
        })
    }

    /// Wraps a linked module (entries `vs_main` and `fs_main`) that samples
    /// `dims` into a program for [`crate::Renderer::register_shader`].
    pub fn program(
        &mut self,
        device: &wgpu::Device,
        module: wgpu::ShaderModule,
        dims: StageDims,
    ) -> ShaderProgram {
        let layout = self.layout(device, dims).1.clone();
        ShaderProgram {
            module,
            layout,
            vs_entry: "vs_main".into(),
            fs_entry: "fs_main".into(),
        }
    }

    /// Group 0: the vertex and pixel uniform buffers, as bytes.
    pub fn uniforms(&self, device: &wgpu::Device, vertex: &[u8], pixel: &[u8]) -> wgpu::BindGroup {
        let buf = |label, contents: &[u8]| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage: wgpu::BufferUsages::UNIFORM,
            })
        };
        let v = buf("program vertex uniforms", vertex);
        let p = buf("program pixel uniforms", pixel);
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("program uniforms"),
            layout: &self.uniforms,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: v.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: p.as_entire_binding(),
                },
            ],
        })
    }

    /// The sampler for `key` (cached).
    pub fn sampler(&mut self, device: &wgpu::Device, key: SamplerKey) -> wgpu::Sampler {
        self.samplers
            .entry(key)
            .or_insert_with(|| device.create_sampler(&key.descriptor()))
            .clone()
    }

    /// A 1x1 opaque-black texture viewed as `dim`.
    pub fn dummy(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        dim: wgpu::TextureViewDimension,
    ) -> wgpu::TextureView {
        self.dummies
            .entry(dim)
            .or_insert_with(|| {
                let (dimension, layers) = match dim {
                    wgpu::TextureViewDimension::D3 => (wgpu::TextureDimension::D3, 1),
                    wgpu::TextureViewDimension::Cube => (wgpu::TextureDimension::D2, 6),
                    _ => (wgpu::TextureDimension::D2, 1),
                };
                let texels: Vec<u8> = [0u8, 0, 0, 255].repeat(layers as usize);
                let t = device.create_texture_with_data(
                    queue,
                    &wgpu::TextureDescriptor {
                        label: Some("program stand-in texture"),
                        size: wgpu::Extent3d {
                            width: 1,
                            height: 1,
                            depth_or_array_layers: layers,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    },
                    wgpu::util::TextureDataOrder::LayerMajor,
                    &texels,
                );
                t.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(dim),
                    ..Default::default()
                })
            })
            .clone()
    }

    /// Group 1 for a program sampling `dims`: `views[n]` (a stand-in when
    /// `None`) with the sampler for `samplers[n]`. `None` when the program
    /// samples nothing. Each view must have the dimension `dims[n]` names
    /// and a filterable float format (see [`filterable`]).
    pub fn textures(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        dims: StageDims,
        views: [Option<&wgpu::TextureView>; 4],
        samplers: [SamplerKey; 4],
    ) -> Option<wgpu::BindGroup> {
        let mut owned: Vec<(u32, wgpu::TextureView, wgpu::Sampler)> = Vec::new();
        for n in 0..4 {
            let Some(d) = dims[n] else { continue };
            let view = match views[n] {
                Some(v) => v.clone(),
                None => self.dummy(device, queue, d),
            };
            let sampler = self.sampler(device, samplers[n]);
            owned.push((n as u32, view, sampler));
        }
        let layout = self.layout(device, dims).0.as_ref()?;
        let mut entries = Vec::new();
        for (n, view, sampler) in &owned {
            entries.push(wgpu::BindGroupEntry {
                binding: 2 * n,
                resource: wgpu::BindingResource::TextureView(view),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 2 * n + 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            });
        }
        Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("program textures"),
            layout,
            entries: &entries,
        }))
    }
}
