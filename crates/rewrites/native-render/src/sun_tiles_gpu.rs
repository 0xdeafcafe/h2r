//! E3 lighting's sun shadows from objects, drawn
//! (docs/graphics/e3-lighting.md, "Sun shadows from objects").
//!
//! [`SunShadows`] owns the persistent depth atlas and the planner of its
//! tiles ([`crate::sun_tiles`]). Each frame it:
//!
//! 1. plans the casters' tiles ([`SunShadows::plan`]);
//! 2. clears and draws only the tiles that changed ([`SunShadows::draw_tiles`]):
//!    each caster's world-space triangles, depth only, through its tile's
//!    orthographic rows, under a viewport and scissor on its rectangle;
//! 3. draws every receiver of every tile it meets ([`SunShadows::draw_receivers`]),
//!    the caster included, which is self-shadowing.
//!
//! The receiver is E3's own program (the design's fallback where a
//! material has no layer-11 receiver), built to retail's implementation 3
//! rules: the vertex gate is the world normal dotted with the direction
//! towards the light, clamped to `[0, 1]`, and the pixel term interpolates
//! between unity and the depth comparison by that gate, times the
//! caster's strength. A triangle whose three gates are zero multiplies by
//! exactly one and keeps the destination's RGB. The comparison is the
//! Sharp edge (`shaders::psh::shadow`, "The edge"): four bilinear taps
//! over a 3x3 texel tent, kept inside the tile's rectangle, then a ramp one
//! screen pixel wide. The term multiplies the frame's RGB; alpha is kept.
//! Receivers draw at the frame's depth (`LessEqual`, no depth write) with
//! the same vertex transform that laid it ([`SunShadows::draw_depth`]).
//!
//! Positions and normals are world space: the caller's skinning supplies
//! the blended rows. The materials' layer-10 depth writers and layer-11
//! receivers are not used here yet; nothing in the game hands this module
//! casters until `FrameScene::shadows` exists (FrameScene slice 3).

use std::collections::HashMap;
use std::num::NonZeroU64;

use wgpu::util::DeviceExt;

use crate::sun_tiles::{SunCaster, SunPlan, SunTilePlanner};

/// The atlas's texel format.
pub const ATLAS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// The per-draw uniform block: clip rows, tile rows, rectangle, light and
/// bias, as `vec4f`s.
const DRAW_BYTES: usize = 4 * 16 + 4 * 16 + 3 * 16;

/// Normal offset of a receiver's tile lookup, in tile texels.
const NORMAL_OFFSET_TEXELS: f32 = 1.5;
/// Constant depth bias of the lookup, in tile texels of depth.
const DEPTH_BIAS_TEXELS: f32 = 1.0;

pub(crate) const WGSL: &str = r#"
struct Draw {
    clip: array<vec4f, 4>,
    tile: array<vec4f, 4>,
    // x, y, width, height in atlas texels.
    rect: vec4f,
    // xyz: unit vector towards the light; w: strength.
    light: vec4f,
    // x: normal offset in world units; y: depth bias.
    bias: vec4f,
};
@group(0) @binding(0) var<uniform> d: Draw;
@group(1) @binding(0) var atlas: texture_depth_2d;

fn apply(m: array<vec4f, 4>, p: vec3f) -> vec4f {
    let q = vec4f(p, 1.0);
    return vec4f(dot(m[0], q), dot(m[1], q), dot(m[2], q), dot(m[3], q));
}

// Invariant, so a receiver meets the depth this laid at equal depth.
@vertex
fn vs_depth(@location(0) p: vec3f) -> @builtin(position) @invariant vec4f {
    return apply(d.clip, p);
}

// A triangle over the whole viewport at the far plane: clears one tile.
@vertex
fn vs_clear(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    return vec4f(x, y, 1.0, 1.0);
}

struct Recv {
    @builtin(position) @invariant pos: vec4f,
    @location(0) tile: vec3f,
    @location(1) gate: f32,
};

@vertex
fn vs_receive(@location(0) p: vec3f, @location(1) n: vec3f) -> Recv {
    var o: Recv;
    o.pos = apply(d.clip, p);
    let len = length(n);
    let nn = select(vec3f(0.0), n / len, len > 1e-6);
    o.gate = clamp(dot(nn, d.light.xyz), 0.0, 1.0);
    o.tile = apply(d.tile, p + nn * d.bias.x).xyz;
    return o;
}

@fragment
fn fs_receive(i: Recv) -> @location(0) vec4f {
    let uv = vec2f(i.tile.x * 0.5 + 0.5, 0.5 - i.tile.y * 0.5);
    let inside = all(uv >= vec2f(0.0)) && all(uv <= vec2f(1.0));
    let r = clamp(i.tile.z, 0.0, 1.0) - d.bias.y;
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
            coverage += wx[col] * wy[row] * select(0.0, 1.0, textureLoad(atlas, vec2i(x, y), 0) >= r);
        }
    }
    let width = max(length(vec2f(dpdxFine(coverage), dpdyFine(coverage))), 1e-6);
    let lit = select(1.0, clamp(0.5 + (coverage - 0.5) / width, 0.0, 1.0), inside);
    let term = 1.0 - i.gate * d.light.w * (1.0 - lit);
    return vec4f(term, term, term, 1.0);
}
"#;

/// A mesh in world space: positions and per-vertex normals.
pub struct SunMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
}

impl SunMesh {
    pub fn new(
        device: &wgpu::Device,
        positions: &[[f32; 3]],
        normals: &[[f32; 3]],
        indices: &[u32],
    ) -> Result<Self, SunRefused> {
        if positions.len() != normals.len() {
            return Err(SunRefused::Mesh("positions and normals differ in length"));
        }
        if indices.is_empty() || indices.len() % 3 != 0 {
            return Err(SunRefused::Mesh("indices are not whole triangles"));
        }
        if indices.iter().any(|&i| i as usize >= positions.len()) {
            return Err(SunRefused::Mesh("an index is past the last vertex"));
        }
        let mut bytes = Vec::with_capacity(positions.len() * 24);
        for (p, n) in positions.iter().zip(normals) {
            for v in p.iter().chain(n) {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sun shadow mesh vertices"),
            contents: &bytes,
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indices_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sun shadow mesh indices"),
            contents: bytemuck::cast_slice(indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        Ok(Self {
            vertices,
            indices: indices_buffer,
            count: indices.len() as u32,
        })
    }
}

/// Why a sun shadow draw was not made. Nothing is encoded for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SunRefused {
    Mesh(&'static str),
    /// A tile to draw has no caster mesh.
    NoCaster(u64),
}

/// The frame a receiver pass multiplies.
pub struct SunTarget<'a> {
    pub colour: &'a wgpu::TextureView,
    pub colour_format: wgpu::TextureFormat,
    /// The depth the receivers were laid in.
    pub depth: &'a wgpu::TextureView,
    pub depth_format: wgpu::TextureFormat,
}

/// One instance or cluster part that may receive sun tiles.
pub struct SunReceiver<'a> {
    pub mesh: &'a SunMesh,
    pub centre: [f32; 3],
    pub radius: f32,
}

#[derive(Clone, Copy)]
struct DrawBlock {
    clip: [[f32; 4]; 4],
    tile: [[f32; 4]; 4],
    rect: [f32; 4],
    light: [f32; 4],
    bias: [f32; 4],
}

impl DrawBlock {
    fn plain(clip: [[f32; 4]; 4]) -> Self {
        Self {
            clip,
            tile: clip,
            rect: [0.0; 4],
            light: [0.0; 4],
            bias: [0.0; 4],
        }
    }

    fn write(&self, out: &mut [u8]) {
        let floats =
            self.clip
                .iter()
                .chain(&self.tile)
                .chain([&self.rect, &self.light, &self.bias]);
        for (i, v) in floats.flatten().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
    }
}

/// The persistent atlas, its planner and the three programs.
pub struct SunShadows {
    planner: SunTilePlanner,
    atlas_view: wgpu::TextureView,
    /// The atlas has never been written.
    fresh: bool,
    module: wgpu::ShaderModule,
    uniforms_layout: wgpu::BindGroupLayout,
    plain_layout: wgpu::PipelineLayout,
    receive_layout: wgpu::PipelineLayout,
    atlas_group: wgpu::BindGroup,
    clear: wgpu::RenderPipeline,
    depth: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    receive: HashMap<(wgpu::TextureFormat, wgpu::TextureFormat), wgpu::RenderPipeline>,
}

const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 2] = [
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x3,
        offset: 0,
        shader_location: 0,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x3,
        offset: 12,
        shader_location: 1,
    },
];

const VERTEX_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: 24,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &VERTEX_ATTRIBUTES,
};

fn depth_state(
    format: wgpu::TextureFormat,
    write: bool,
    compare: wgpu::CompareFunction,
) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format,
        depth_write_enabled: Some(write),
        depth_compare: Some(compare),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    }
}

impl SunShadows {
    /// `atlas_edge` is a power of two, [`crate::sun_tiles::ATLAS_EDGE`] in
    /// the game.
    pub fn new(device: &wgpu::Device, atlas_edge: u32) -> Self {
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sun shadow atlas"),
            size: wgpu::Extent3d {
                width: atlas_edge,
                height: atlas_edge,
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
            label: Some("sun shadows"),
            source: wgpu::ShaderSource::Wgsl(WGSL.into()),
        });
        let uniforms_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sun shadow draw"),
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
            label: Some("sun shadow atlas"),
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
            label: Some("sun shadow depth"),
            bind_group_layouts: &[Some(&uniforms_layout)],
            immediate_size: 0,
        });
        let receive_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sun shadow receiver"),
            bind_group_layouts: &[Some(&uniforms_layout), Some(&atlas_layout)],
            immediate_size: 0,
        });
        let atlas_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sun shadow atlas"),
            layout: &atlas_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&atlas_view),
            }],
        });
        let clear = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sun tile clear"),
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
            planner: SunTilePlanner::new(atlas_edge),
            atlas_view,
            fresh: true,
            module,
            uniforms_layout,
            plain_layout,
            receive_layout,
            atlas_group,
            clear,
            depth: HashMap::new(),
            receive: HashMap::new(),
        }
    }

    pub fn atlas_edge(&self) -> u32 {
        self.planner.atlas_edge()
    }

    /// Plans this frame's tiles against the atlas's contents.
    pub fn plan(&mut self, casters: &[SunCaster]) -> SunPlan {
        self.planner.plan(casters)
    }

    fn depth_pipeline(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> wgpu::RenderPipeline {
        let (module, layout) = (&self.module, &self.plain_layout);
        self.depth
            .entry(format)
            .or_insert_with(|| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("sun shadow depth"),
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
                    depth_stencil: Some(depth_state(format, true, wgpu::CompareFunction::Less)),
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
                    label: Some("sun shadow receiver"),
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
                            // The frame times the term; alpha is kept.
                            blend: Some(wgpu::BlendState {
                                color: wgpu::BlendComponent {
                                    src_factor: wgpu::BlendFactor::Dst,
                                    dst_factor: wgpu::BlendFactor::Zero,
                                    operation: wgpu::BlendOperation::Add,
                                },
                                alpha: wgpu::BlendComponent {
                                    src_factor: wgpu::BlendFactor::Zero,
                                    dst_factor: wgpu::BlendFactor::One,
                                    operation: wgpu::BlendOperation::Add,
                                },
                            }),
                            write_mask: wgpu::ColorWrites::COLOR,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            })
            .clone()
    }

    /// One uniform buffer holding `blocks` at the device's dynamic-offset
    /// alignment, its bind group and the stride.
    fn uniforms(&self, device: &wgpu::Device, blocks: &[DrawBlock]) -> (wgpu::BindGroup, u32) {
        let align = device.limits().min_uniform_buffer_offset_alignment.max(1) as usize;
        let stride = DRAW_BYTES.div_ceil(align) * align;
        let mut bytes = vec![0u8; stride * blocks.len().max(1)];
        for (i, b) in blocks.iter().enumerate() {
            b.write(&mut bytes[i * stride..i * stride + DRAW_BYTES]);
        }
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sun shadow draws"),
            contents: &bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sun shadow draws"),
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

    /// Clears and draws the tiles `plan` marks for redrawing, each from
    /// its caster's mesh in `casters`. A tile kept from an earlier frame is
    /// not touched. Counts the caster draws in `plan.work.tile_draws`.
    pub fn draw_tiles(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        plan: &mut SunPlan,
        casters: &[(u64, &SunMesh)],
    ) -> Result<(), SunRefused> {
        let mesh_of = |id: u64| casters.iter().find(|(c, _)| *c == id).map(|(_, m)| *m);
        let mut work = Vec::new();
        for tile in plan.tiles.iter().filter(|t| t.redraw) {
            let Some(mesh) = mesh_of(tile.id) else {
                // The planner thinks the tile is drawn; it is not.
                self.planner.invalidate();
                return Err(SunRefused::NoCaster(tile.id));
            };
            work.push((tile, mesh));
        }
        if work.is_empty() && !self.fresh {
            return Ok(());
        }
        let blocks: Vec<DrawBlock> = work.iter().map(|(t, _)| DrawBlock::plain(t.rows)).collect();
        let (group, stride) = self.uniforms(device, &blocks);
        let depth = self.depth_pipeline(device, ATLAS_FORMAT);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sun tiles"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.atlas_view,
                depth_ops: Some(wgpu::Operations {
                    load: if self.fresh {
                        wgpu::LoadOp::Clear(1.0)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        for (i, (tile, mesh)) in work.iter().enumerate() {
            let [x, y, w, h] = tile.rect;
            pass.set_viewport(x as f32, y as f32, w as f32, h as f32, 0.0, 1.0);
            pass.set_scissor_rect(x, y, w, h);
            pass.set_bind_group(0, &group, &[i as u32 * stride]);
            pass.set_pipeline(&self.clear);
            pass.draw(0..3, 0..1);
            pass.set_pipeline(&depth);
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.count, 0, 0..1);
        }
        drop(pass);
        self.fresh = false;
        plan.work.tile_draws += work.len() as u32;
        Ok(())
    }

    /// Lays `meshes`' depth under `clip` rows into `depth`, with the vertex
    /// transform the receivers use, so they meet it at equal depth.
    pub fn draw_depth(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        depth: (&wgpu::TextureView, wgpu::TextureFormat),
        clip: [[f32; 4]; 4],
        meshes: &[&SunMesh],
        clear: bool,
    ) {
        let (view, format) = depth;
        let (group, _) = self.uniforms(device, &[DrawBlock::plain(clip)]);
        let pipeline = self.depth_pipeline(device, format);
        let load = |v: f32| {
            if clear {
                wgpu::LoadOp::Clear(v)
            } else {
                wgpu::LoadOp::Load
            }
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sun shadow receiver depth"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view,
                depth_ops: Some(wgpu::Operations {
                    load: load(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: format.has_stencil_aspect().then_some(wgpu::Operations {
                    load: if clear {
                        wgpu::LoadOp::Clear(0)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                }),
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[0]);
        for mesh in meshes {
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.count, 0, 0..1);
        }
    }

    /// Multiplies `target` by the sun shadow of every tile each receiver
    /// meets. Counts the draws in `plan.work.receiver_draws`.
    pub fn draw_receivers(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &SunTarget<'_>,
        clip: [[f32; 4]; 4],
        plan: &mut SunPlan,
        receivers: &[SunReceiver<'_>],
    ) {
        let mut blocks = Vec::new();
        let mut draws = Vec::new();
        for r in receivers {
            for tile in plan.tiles.iter().filter(|t| t.meets(r.centre, r.radius)) {
                let [x, y, w, h] = tile.rect;
                let l = tile.towards_light;
                draws.push(r.mesh);
                blocks.push(DrawBlock {
                    clip,
                    tile: tile.rows,
                    rect: [x as f32, y as f32, w as f32, h as f32],
                    light: [l[0], l[1], l[2], tile.strength],
                    bias: [
                        NORMAL_OFFSET_TEXELS * tile.texel(),
                        DEPTH_BIAS_TEXELS / tile.edge as f32,
                        0.0,
                        0.0,
                    ],
                });
            }
        }
        if draws.is_empty() {
            return;
        }
        let (group, stride) = self.uniforms(device, &blocks);
        let pipeline = self.receive_pipeline(device, target.colour_format, target.depth_format);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sun shadow receivers"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target.colour,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: target.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: target
                    .depth_format
                    .has_stencil_aspect()
                    .then_some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(1, &self.atlas_group, &[]);
        for (i, mesh) in draws.iter().enumerate() {
            pass.set_bind_group(0, &group, &[i as u32 * stride]);
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.count, 0, 0..1);
        }
        drop(pass);
        plan.work.receiver_draws += draws.len() as u32;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_sun_program_validates() {
        shaders::validate(super::WGSL).expect("sun shadow WGSL");
    }
}
