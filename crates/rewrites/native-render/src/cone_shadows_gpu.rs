//! E3 lighting's shadows from cones, drawn (docs/graphics/e3-lighting.md,
//! "Shadows from local lights").
//!
//! [`ConeShadows`] owns the local lights' depth atlas
//! ([`crate::cone_shadows::ATLAS_EDGE`], four slots). Per frame, for each
//! cone the renderer shadows ([`crate::cone_shadows::choose`]):
//!
//! 1. its tile: the slot cleared, then the casters' world-space triangles,
//!    depth only, through the cone's perspective rows
//!    ([`ConeShadows::prepare_tiles`]). Cones move with their owners, so
//!    every tile is drawn every frame;
//! 2. its receivers' shadow term into the frame's destination alpha
//!    ([`ConeShadows::prepare_receivers`]), which is what retail's layer 11
//!    writes for the light's later passes to read (`0x00031348`; the
//!    shadowed implementation of layer 12 reads it, **S**). The term
//!    multiplies the alpha; RGB is kept.
//!
//! The term follows the sun's receiver (`sun_tiles_gpu`): a vertex gate
//! of the world normal dotted with the direction to the apex, clamped to
//! `[0, 1]`, and the Sharp edge's four bilinear taps over a 3x3 texel tent
//! kept inside the slot, then a ramp one screen pixel wide. The tile's
//! depth is projective; the comparison is in linear depth from the apex,
//! with a bias of one texel's width at the receiver's depth, so it holds
//! as well at the far end of a lamp's cone as at the near.
//!
//! Positions and normals are world space; the caller skins them
//! (`sun_casters::skin`) and uploads them as [`SunMesh`]es.

use std::collections::HashMap;
use std::num::NonZeroU64;

use wgpu::util::DeviceExt;

use crate::cone_shadows::{ATLAS_EDGE, Cone};
use crate::sun_tiles_gpu::{SunFrame, SunMesh, SunReceiver, VERTEX_LAYOUT, depth_state};

/// The atlas's texel format.
pub const ATLAS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// The per-draw uniform block: clip rows, tile rows, rectangle, light,
/// bias and depth rule, as `vec4f`s.
const DRAW_BYTES: usize = 4 * 16 + 4 * 16 + 4 * 16;

/// Normal offset of a receiver's tile lookup, in tile texels.
const NORMAL_OFFSET_TEXELS: f32 = 1.5;
/// Depth bias of the comparison, in tile texels at the receiver's depth.
const DEPTH_BIAS_TEXELS: f32 = 1.0;

pub(crate) const WGSL: &str = r#"
struct Draw {
    clip: array<vec4f, 4>,
    tile: array<vec4f, 4>,
    // x, y, width, height in atlas texels.
    rect: vec4f,
    // xyz: the cone's apex; w: strength.
    light: vec4f,
    // x: normal offset and y: depth bias, each in world units per unit of
    // depth from the apex; z: far / (far - near); w: near.
    bias: vec4f,
    // The guest's W-buffer, as the sun's receiver takes it.
    depth: vec4f,
};
@group(0) @binding(0) var<uniform> d: Draw;
@group(1) @binding(0) var atlas: texture_depth_2d;

fn apply(m: array<vec4f, 4>, p: vec3f) -> vec4f {
    let q = vec4f(p, 1.0);
    return vec4f(dot(m[0], q), dot(m[1], q), dot(m[2], q), dot(m[3], q));
}

@vertex
fn vs_depth(@location(0) p: vec3f) -> @builtin(position) @invariant vec4f {
    return apply(d.clip, p);
}

// A triangle over the whole viewport at the far plane: clears one slot.
@vertex
fn vs_clear(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    return vec4f(x, y, 1.0, 1.0);
}

struct Recv {
    @builtin(position) @invariant pos: vec4f,
    // The tile's clip position, divided per fragment.
    @location(0) tile: vec4f,
    @location(1) gate: f32,
};

@vertex
fn vs_receive(@location(0) p: vec3f, @location(1) n: vec3f) -> Recv {
    var o: Recv;
    o.pos = apply(d.clip, p);
    let len = length(n);
    let nn = select(vec3f(0.0), n / len, len > 1e-6);
    let to_apex = d.light.xyz - p;
    let reach = length(to_apex);
    let l = select(vec3f(0.0), to_apex / reach, reach > 1e-6);
    o.gate = clamp(dot(nn, l), 0.0, 1.0);
    let depth = max(apply(d.tile, p).w, 0.0);
    o.tile = apply(d.tile, p + nn * d.bias.x * depth);
    return o;
}

struct Term {
    @location(0) colour: vec4f,
    @builtin(frag_depth) depth: f32,
};

fn receiver_depth(pos: vec4f) -> f32 {
    if d.depth.x <= 0.0 {
        return pos.z;
    }
    let w = max(1.0 / pos.w - d.depth.w, 0.0);
    let z = w * d.depth.x * d.depth.y;
    let q = select(z, floor(z), d.depth.z != 0.0);
    return clamp(q / d.depth.x, 0.0, 1.0);
}

// Linear depth from the apex of a tile texel's projective depth.
fn linear(z: f32) -> f32 {
    return d.bias.z * d.bias.w / max(d.bias.z - z, 1e-6);
}

@fragment
fn fs_receive(i: Recv) -> Term {
    let w = i.tile.w;
    let t = i.tile.xyz / max(w, 1e-6);
    let uv = vec2f(t.x * 0.5 + 0.5, 0.5 - t.y * 0.5);
    let inside = w > 0.0 && all(uv >= vec2f(0.0)) && all(uv <= vec2f(1.0)) && t.z <= 1.0;
    let r = w - d.bias.y * w;
    let lo = vec2i(d.rect.xy);
    let hi = lo + vec2i(d.rect.zw) - vec2i(1);
    let p = d.rect.xy + clamp(uv, vec2f(0.0), vec2f(1.0)) * d.rect.zw;
    let base = vec2i(floor(p));
    let f = fract(p);
    let wx = vec3f(1.0 - f.x, 1.0, f.x) * 0.5;
    let wy = vec3f(1.0 - f.y, 1.0, f.y) * 0.5;
    var coverage = 0.0;
    for (var row = 0; row < 3; row++) {
        let y = clamp(base.y + row - 1, lo.y, hi.y);
        for (var col = 0; col < 3; col++) {
            let x = clamp(base.x + col - 1, lo.x, hi.x);
            let seen = linear(textureLoad(atlas, vec2i(x, y), 0)) >= r;
            coverage += wx[col] * wy[row] * select(0.0, 1.0, seen);
        }
    }
    let width = max(length(vec2f(dpdxFine(coverage), dpdyFine(coverage))), 1e-6);
    let lit = select(1.0, clamp(0.5 + (coverage - 0.5) / width, 0.0, 1.0), inside);
    let term = 1.0 - i.gate * d.light.w * (1.0 - lit);
    return Term(vec4f(0.0, 0.0, 0.0, term), receiver_depth(i.pos));
}
"#;

/// One cone's tile: its slot and projection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConeTile {
    /// `x, y, width, height` in atlas texels
    /// (`cone_shadows::ShadowedCone::rect`).
    pub rect: [u32; 4],
    pub cone: Cone,
    /// The shadow's strength, in `[0, 1]`.
    pub strength: f32,
}

impl ConeTile {
    fn block(&self, clip: [[f32; 4]; 4], depth: [f32; 4]) -> DrawBlock {
        let [x, y, w, h] = self.rect;
        let a = self.cone.apex;
        // World units per texel per unit of depth from the apex.
        let texel = self.cone.texel(1.0, w.max(h));
        let k = self.cone.far / (self.cone.far - self.cone.near);
        DrawBlock {
            clip,
            tile: self.cone.rows(),
            rect: [x as f32, y as f32, w as f32, h as f32],
            light: [a[0], a[1], a[2], self.strength.clamp(0.0, 1.0)],
            bias: [
                NORMAL_OFFSET_TEXELS * texel,
                DEPTH_BIAS_TEXELS * texel,
                k,
                self.cone.near,
            ],
            depth,
        }
    }
}

#[derive(Clone, Copy)]
struct DrawBlock {
    clip: [[f32; 4]; 4],
    tile: [[f32; 4]; 4],
    rect: [f32; 4],
    light: [f32; 4],
    bias: [f32; 4],
    depth: [f32; 4],
}

impl DrawBlock {
    fn plain(clip: [[f32; 4]; 4]) -> Self {
        Self {
            clip,
            tile: clip,
            rect: [0.0; 4],
            light: [0.0; 4],
            bias: [0.0; 4],
            depth: [0.0; 4],
        }
    }

    fn write(&self, out: &mut [u8]) {
        let floats = self.clip.iter().chain(&self.tile).chain([
            &self.rect,
            &self.light,
            &self.bias,
            &self.depth,
        ]);
        for (i, v) in floats.flatten().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
    }
}

/// The local lights' atlas and the three programs.
pub struct ConeShadows {
    atlas_view: wgpu::TextureView,
    module: wgpu::ShaderModule,
    uniforms_layout: wgpu::BindGroupLayout,
    plain_layout: wgpu::PipelineLayout,
    receive_layout: wgpu::PipelineLayout,
    atlas_group: wgpu::BindGroup,
    clear: wgpu::RenderPipeline,
    depth: Option<wgpu::RenderPipeline>,
    receive: HashMap<(wgpu::TextureFormat, wgpu::TextureFormat), wgpu::RenderPipeline>,
}

impl ConeShadows {
    /// An atlas of [`ATLAS_EDGE`] texels a side.
    pub fn new(device: &wgpu::Device) -> Self {
        Self::with_edge(device, ATLAS_EDGE)
    }

    /// An atlas of `edge` texels a side: tests use smaller ones.
    pub fn with_edge(device: &wgpu::Device, edge: u32) -> Self {
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cone shadow atlas"),
            size: wgpu::Extent3d {
                width: edge,
                height: edge,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: ATLAS_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let atlas_view = atlas.create_view(&Default::default());
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cone shadows"),
            source: wgpu::ShaderSource::Wgsl(WGSL.into()),
        });
        let uniforms_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cone shadow draw"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: NonZeroU64::new(DRAW_BYTES as u64),
                },
                count: None,
            }],
        });
        let atlas_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cone shadow atlas"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let plain_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cone shadow depth"),
            bind_group_layouts: &[Some(&uniforms_layout)],
            immediate_size: 0,
        });
        let receive_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cone shadow receiver"),
            bind_group_layouts: &[Some(&uniforms_layout), Some(&atlas_layout)],
            immediate_size: 0,
        });
        let atlas_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cone shadow atlas"),
            layout: &atlas_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&atlas_view),
            }],
        });
        let clear = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cone tile clear"),
            layout: Some(&plain_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_clear"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(depth_state(
                ATLAS_FORMAT,
                true,
                wgpu::CompareFunction::Always,
            )),
            multisample: wgpu::MultisampleState::default(),
            fragment: None,
            multiview_mask: None,
            cache: None,
        });
        Self {
            atlas_view,
            module,
            uniforms_layout,
            plain_layout,
            receive_layout,
            atlas_group,
            clear,
            depth: None,
            receive: HashMap::new(),
        }
    }

    fn depth_pipeline(&mut self, device: &wgpu::Device) -> wgpu::RenderPipeline {
        let (module, layout) = (&self.module, &self.plain_layout);
        self.depth
            .get_or_insert_with(|| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("cone shadow depth"),
                    layout: Some(layout),
                    vertex: wgpu::VertexState {
                        module,
                        entry_point: Some("vs_depth"),
                        compilation_options: Default::default(),
                        buffers: &[Some(VERTEX_LAYOUT)],
                    },
                    primitive: wgpu::PrimitiveState {
                        cull_mode: None,
                        ..Default::default()
                    },
                    depth_stencil: Some(depth_state(
                        ATLAS_FORMAT,
                        true,
                        wgpu::CompareFunction::Less,
                    )),
                    multisample: wgpu::MultisampleState::default(),
                    fragment: None,
                    multiview_mask: None,
                    cache: None,
                })
            })
            .clone()
    }

    fn receive_pipeline(
        &mut self,
        device: &wgpu::Device,
        colour: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
    ) -> wgpu::RenderPipeline {
        let (module, layout) = (&self.module, &self.receive_layout);
        self.receive
            .entry((colour, depth))
            .or_insert_with(|| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("cone shadow receiver"),
                    layout: Some(layout),
                    vertex: wgpu::VertexState {
                        module,
                        entry_point: Some("vs_receive"),
                        compilation_options: Default::default(),
                        buffers: &[Some(VERTEX_LAYOUT)],
                    },
                    primitive: wgpu::PrimitiveState {
                        cull_mode: None,
                        ..Default::default()
                    },
                    depth_stencil: Some(depth_state(
                        depth,
                        false,
                        wgpu::CompareFunction::LessEqual,
                    )),
                    multisample: wgpu::MultisampleState::default(),
                    fragment: Some(wgpu::FragmentState {
                        module,
                        entry_point: Some("fs_receive"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: colour,
                            // The destination alpha times the term; RGB
                            // is kept.
                            blend: Some(wgpu::BlendState {
                                color: wgpu::BlendComponent {
                                    src_factor: wgpu::BlendFactor::Zero,
                                    dst_factor: wgpu::BlendFactor::One,
                                    operation: wgpu::BlendOperation::Add,
                                },
                                alpha: wgpu::BlendComponent {
                                    src_factor: wgpu::BlendFactor::Zero,
                                    dst_factor: wgpu::BlendFactor::SrcAlpha,
                                    operation: wgpu::BlendOperation::Add,
                                },
                            }),
                            write_mask: wgpu::ColorWrites::ALPHA,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            })
            .clone()
    }

    fn uniforms(&self, device: &wgpu::Device, blocks: &[DrawBlock]) -> (wgpu::BindGroup, u32) {
        let align = device.limits().min_uniform_buffer_offset_alignment.max(1) as usize;
        let stride = DRAW_BYTES.div_ceil(align) * align;
        let mut bytes = vec![0u8; stride * blocks.len().max(1)];
        for (i, b) in blocks.iter().enumerate() {
            b.write(&mut bytes[i * stride..i * stride + DRAW_BYTES]);
        }
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cone shadow draws"),
            contents: &bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cone shadow draws"),
            layout: &self.uniforms_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buffer,
                    offset: 0,
                    size: NonZeroU64::new(DRAW_BYTES as u64),
                }),
            }],
        });
        (group, stride as u32)
    }

    /// The tiles' pass: each tile's slot cleared and its casters drawn.
    /// `None` when there is no tile.
    pub fn prepare_tiles(
        &mut self,
        device: &wgpu::Device,
        tiles: &[(ConeTile, Vec<SunMesh>)],
    ) -> Option<ConeTilePass> {
        if tiles.is_empty() {
            return None;
        }
        let blocks: Vec<DrawBlock> = tiles
            .iter()
            .map(|(t, _)| DrawBlock::plain(t.cone.rows()))
            .collect();
        let (group, stride) = self.uniforms(device, &blocks);
        let depth = self.depth_pipeline(device);
        Some(ConeTilePass {
            atlas: self.atlas_view.clone(),
            clear: self.clear.clone(),
            depth,
            group,
            stride,
            work: tiles
                .iter()
                .map(|(t, casters)| (t.rect, casters.clone()))
                .collect(),
        })
    }

    /// The receivers of `tile` on a frame whose attachments have
    /// `formats` (colour, depth): each multiplies the destination alpha
    /// by its shadow term. `None` when there is no receiver.
    pub fn prepare_receivers(
        &mut self,
        device: &wgpu::Device,
        formats: (wgpu::TextureFormat, wgpu::TextureFormat),
        frame: &SunFrame,
        tile: &ConeTile,
        receivers: &[SunReceiver<'_>],
    ) -> Option<ConeReceiverPass> {
        let draws: Vec<SunMesh> = receivers
            .iter()
            .filter(|r| tile.cone.meets(r.centre, r.radius))
            .map(|r| r.mesh.clone())
            .collect();
        if draws.is_empty() {
            return None;
        }
        let block = tile.block(frame.clip, frame.depth);
        let (group, _) = self.uniforms(device, &[block]);
        let pipeline = self.receive_pipeline(device, formats.0, formats.1);
        Some(ConeReceiverPass {
            pipeline,
            atlas_group: self.atlas_group.clone(),
            group,
            draws,
            viewport: frame.viewport,
            stencil: formats.1.has_stencil_aspect(),
        })
    }
}

/// The tiles' pass, prepared ([`ConeShadows::prepare_tiles`]).
pub struct ConeTilePass {
    atlas: wgpu::TextureView,
    clear: wgpu::RenderPipeline,
    depth: wgpu::RenderPipeline,
    group: wgpu::BindGroup,
    stride: u32,
    work: Vec<([u32; 4], Vec<SunMesh>)>,
}

impl ConeTilePass {
    /// Caster draws the pass makes.
    pub fn draws(&self) -> usize {
        self.work.iter().map(|(_, m)| m.len()).sum()
    }

    pub fn record(&self, encoder: &mut wgpu::CommandEncoder) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("cone tiles"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.atlas,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        for (i, (rect, meshes)) in self.work.iter().enumerate() {
            let [x, y, w, h] = *rect;
            pass.set_viewport(x as f32, y as f32, w as f32, h as f32, 0.0, 1.0);
            pass.set_scissor_rect(x, y, w, h);
            pass.set_bind_group(0, &self.group, &[i as u32 * self.stride]);
            pass.set_pipeline(&self.clear);
            pass.draw(0..3, 0..1);
            pass.set_pipeline(&self.depth);
            for mesh in meshes {
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.count, 0, 0..1);
            }
        }
    }
}

/// One tile's receiver pass, prepared ([`ConeShadows::prepare_receivers`]).
pub struct ConeReceiverPass {
    pipeline: wgpu::RenderPipeline,
    atlas_group: wgpu::BindGroup,
    group: wgpu::BindGroup,
    draws: Vec<SunMesh>,
    viewport: Option<[f32; 4]>,
    stencil: bool,
}

impl ConeReceiverPass {
    /// Receiver draws the pass makes.
    pub fn draws(&self) -> usize {
        self.draws.len()
    }

    /// Multiplies `colour`'s alpha where `depth` holds the receivers.
    pub fn record(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        colour: &wgpu::TextureView,
        depth: &wgpu::TextureView,
    ) {
        fn kept<V>() -> wgpu::Operations<V> {
            wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            }
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("cone shadow receivers"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: colour,
                depth_slice: None,
                resolve_target: None,
                ops: kept(),
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(kept()),
                stencil_ops: self.stencil.then(kept),
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if let Some([x, y, w, h]) = self.viewport {
            pass.set_viewport(x, y, w, h, 0.0, 1.0);
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.group, &[0]);
        pass.set_bind_group(1, &self.atlas_group, &[]);
        for mesh in &self.draws {
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.count, 0, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_cone_program_validates() {
        shaders::validate(super::WGSL).expect("cone shadow WGSL");
    }
}
