//! Resources for translated Xbox shader programs.
//!
//! `crates/shaders` emits one WGSL module per vertex/pixel pair with a fixed
//! resource contract (its `docs/graphics/shaders.md`):
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
//! makes the per-draw bind groups: the uniforms, and the stage textures (a
//! 1x1 stand-in, opaque black, for a stage the program samples but nothing
//! usable is bound to) with samplers cached by [`SamplerKey`].
//!
//! Uniforms are staged in an [`UploadRing`] and bound with dynamic offsets
//! into one group per ring buffer, so a draw costs no buffer or bind group
//! of its own: call [`ProgramBindings::flush_uploads`] before the submit
//! that runs the draws and [`ProgramBindings::submitted`] after it. Texture
//! groups are cached by view and sampler; [`ProgramBindings::end_frame`]
//! drops the ones not used for a few frames.

use std::collections::HashMap;

use wgpu::util::DeviceExt;

use crate::pipeline::ShaderProgram;
use crate::renderer::BoundGroup;
use crate::ring::UploadRing;
use crate::translate::SamplerKey;

/// Initial size of the uniform ring.
const UNIFORM_RING: u64 = 4 << 20;
/// Frames a texture group may go unused before [`ProgramBindings::end_frame`]
/// drops it (and the views it holds).
const GROUP_FRAMES: u64 = 4;

/// A texture group's dimensions and, per sampled stage, the view bound (a
/// stand-in when `None`) and its sampler.
type TextureGroupKey = (
    StageDims,
    [Option<(Option<wgpu::TextureView>, SamplerKey, [bool; 4])>; 4],
);

/// Texture dimension per stage a program samples (`None`: not sampled).
pub type StageDims = [Option<wgpu::TextureViewDimension>; 4];

/// Layouts, samplers and stand-in textures for translated programs.
pub struct ProgramBindings {
    uniforms: wgpu::BindGroupLayout,
    layouts: HashMap<StageDims, (Option<wgpu::BindGroupLayout>, wgpu::PipelineLayout)>,
    samplers: HashMap<SamplerKey, wgpu::Sampler>,
    dummies: HashMap<(wgpu::TextureViewDimension, [bool; 4]), wgpu::TextureView>,
    ring: Option<UploadRing>,
    /// Group 0 on the ring's current buffer: (ring generation, vertex and
    /// pixel sizes, group).
    uniform_group: Option<(u64, u64, u64, wgpu::BindGroup)>,
    /// The last vertex and pixel uniforms staged (bytes, offset, ring
    /// generation) since the last submit, for draws that repeat them.
    last: [Option<(Vec<u8>, u32, u64)>; 2],
    uniform_align: u64,
    texture_groups: HashMap<TextureGroupKey, (Option<wgpu::BindGroup>, u64)>,
    frame: u64,
}

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: true,
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

/// Validate a newly created GPU resource. The native backend has the
/// result when the scope is popped. Callers cache the result with the resource.
pub fn validated_resource<T>(device: &wgpu::Device, create: impl FnOnce() -> T) -> Option<T> {
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let resource = create();
    let mut result = std::pin::pin!(scope.pop());
    match std::future::Future::poll(
        result.as_mut(),
        &mut std::task::Context::from_waker(std::task::Waker::noop()),
    ) {
        std::task::Poll::Ready(None) => Some(resource),
        std::task::Poll::Ready(Some(error)) => {
            eprintln!("gfx: GPU resource refused: {error}");
            None
        }
        std::task::Poll::Pending => None,
    }
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
            ring: None,
            uniform_group: None,
            last: [None, None],
            uniform_align: (device.limits().min_uniform_buffer_offset_alignment as u64)
                .max(4)
                .next_power_of_two(),
            texture_groups: HashMap::new(),
            frame: 0,
        }
    }

    /// Group 0 shared by every program linked with these bindings.
    pub fn uniform_layout(&self) -> &wgpu::BindGroupLayout {
        &self.uniforms
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
            count_entry: None,
        }
    }

    /// Group 0 with the vertex and pixel uniforms (as bytes) at its
    /// dynamic offsets. The bytes are staged: they reach the GPU at
    /// [`flush_uploads`](Self::flush_uploads), which must come before the
    /// submit of the draw that binds the group. Uniforms equal to the
    /// previous draw's share its copy.
    pub fn uniforms(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vertex: &[u8],
        pixel: &[u8],
    ) -> BoundGroup {
        let ring = self.ring.get_or_insert_with(|| {
            UploadRing::new(
                device,
                "program uniforms",
                wgpu::BufferUsages::UNIFORM,
                UNIFORM_RING,
            )
        });
        let mut offsets = [0u32; 2];
        loop {
            let generation = ring.generation();
            for (n, bytes) in [vertex, pixel].into_iter().enumerate() {
                if let Some((b, off, g)) = &self.last[n]
                    && *g == generation
                    && b.as_slice() == bytes
                {
                    offsets[n] = *off;
                    continue;
                }
                let (_, off) = ring.push(device, queue, bytes, self.uniform_align);
                offsets[n] = off as u32;
                self.last[n] = Some((bytes.to_vec(), off as u32, ring.generation()));
            }
            // A push that moved to a new buffer leaves the earlier offset in
            // the old one: stage both again.
            if ring.generation() == generation {
                break;
            }
        }
        let (vlen, plen) = (vertex.len() as u64, pixel.len() as u64);
        let generation = ring.generation();
        let group = match &self.uniform_group {
            Some((g, v, p, group)) if *g == generation && *v == vlen && *p == plen => group.clone(),
            _ => {
                let buffer = ring.buffer();
                let binding = |binding, size| wgpu::BindGroupEntry {
                    binding,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(size),
                    }),
                };
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("program uniforms"),
                    layout: &self.uniforms,
                    entries: &[binding(0, vlen.max(4)), binding(1, plen.max(4))],
                });
                self.uniform_group = Some((generation, vlen, plen, group.clone()));
                group
            }
        };
        BoundGroup {
            index: 0,
            group,
            offsets: offsets.to_vec(),
        }
    }

    /// Writes the uniforms staged since the last flush to the queue.
    pub fn flush_uploads(&mut self, queue: &wgpu::Queue) {
        if let Some(r) = &mut self.ring {
            r.flush(queue);
        }
    }

    /// Lets the uniform ring reuse its space: call after the submit that
    /// follows [`flush_uploads`](Self::flush_uploads).
    pub fn submitted(&mut self) {
        if let Some(r) = &mut self.ring {
            r.reset();
        }
        self.last = [None, None];
    }

    /// A frame ended: texture groups unused for a few frames are dropped,
    /// releasing the views (and textures) they hold.
    pub fn end_frame(&mut self) {
        self.frame += 1;
        let frame = self.frame;
        self.texture_groups
            .retain(|_, (_, used)| frame - *used <= GROUP_FRAMES);
    }

    /// The sampler for `key` (cached).
    pub fn sampler(&mut self, device: &wgpu::Device, key: SamplerKey) -> wgpu::Sampler {
        self.samplers
            .entry(key)
            .or_insert_with(|| device.create_sampler(&key.descriptor()))
            .clone()
    }

    /// A 1x1 opaque-black texture viewed as `dim`. `signed` (R, G, B, A)
    /// stores those channels biased, as `texture::DecodeOptions::signed`
    /// does, so a pixel shader reading them signed still sees black
    /// (0x80 reads 0) and an alpha of 254/255, the largest signed value.
    pub fn dummy(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        dim: wgpu::TextureViewDimension,
        signed: [bool; 4],
    ) -> wgpu::TextureView {
        self.dummies
            .entry((dim, signed))
            .or_insert_with(|| {
                let (dimension, layers) = match dim {
                    wgpu::TextureViewDimension::D3 => (wgpu::TextureDimension::D3, 1),
                    wgpu::TextureViewDimension::Cube => (wgpu::TextureDimension::D2, 6),
                    _ => (wgpu::TextureDimension::D2, 1),
                };
                let texel: [u8; 4] = std::array::from_fn(|c| match (c, signed[c]) {
                    (3, _) => 255,
                    (_, true) => 0x80,
                    _ => 0,
                });
                let texels: Vec<u8> = texel.repeat(layers as usize);
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
    /// and a filterable float format (see [`filterable`]). `signed[n]` is
    /// stage `n`'s signed channels, for its stand-in ([`Self::dummy`]).
    pub fn textures(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        dims: StageDims,
        views: [Option<&wgpu::TextureView>; 4],
        samplers: [SamplerKey; 4],
        signed: [[bool; 4]; 4],
    ) -> Option<wgpu::BindGroup> {
        let key: TextureGroupKey = (
            dims,
            std::array::from_fn(|n| dims[n].map(|_| (views[n].cloned(), samplers[n], signed[n]))),
        );
        if let Some((group, used)) = self.texture_groups.get_mut(&key) {
            *used = self.frame;
            return group.clone();
        }
        let mut owned: Vec<(u32, wgpu::TextureView, wgpu::Sampler)> = Vec::new();
        for n in 0..4 {
            let Some(d) = dims[n] else { continue };
            let view = match views[n] {
                Some(v) => v.clone(),
                None => self.dummy(device, queue, d, signed[n]),
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
        let group = validated_resource(device, || device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("program textures"),
            layout,
            entries: &entries,
        }));
        self.texture_groups.insert(key, (group.clone(), self.frame));
        group
    }
}
