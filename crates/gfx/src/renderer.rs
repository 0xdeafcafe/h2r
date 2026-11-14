//! The draw path: guest-style draws through the state model.
//!
//! This is the seed of the D3D device replacement: callers describe a draw
//! with a vertex format ([`VertexLayout`]), vertex streams, a
//! [`PrimitiveType`] and a typed [`RenderState`]; the renderer repacks vertex
//! data where wgpu cannot read the guest encoding, translates the state to a
//! cached wgpu pipeline and records the draw.
//!
//! One shader is built in ([`SHADER_XYZRHW_DIFFUSE`], the fixed-function
//! stand-in for pre-transformed `XYZRHW | DIFFUSE` vertices). Generated
//! shaders plug in through [`Renderer::register_shader`] and
//! [`Draw::shader`].

use std::collections::HashMap;

use wgpu::util::DeviceExt;

use crate::context::GfxContext;
use crate::pipeline::{PipelineCache, PipelineKey, ShaderId, ShaderProgram, TargetFormats, create_render_pipeline};
use crate::presenter::RenderTarget;
use crate::repack::{RepackError, RepackPlan};
use crate::state::RenderState;
use crate::translate::{self, Capabilities};
use crate::vertex::{HostPrimitive, PrimitiveType, VertexDataType, VertexLayout, VertexRegister};

/// The shader id of the built-in `XYZRHW | DIFFUSE` pass-through program.
pub const SHADER_XYZRHW_DIFFUSE: ShaderId = ShaderId(0);

/// Why a draw was rejected.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DrawError {
    /// The built-in shader needs pre-transformed float4 position and a
    /// `D3DCOLOR` diffuse.
    #[error("the built-in shader needs XYZRHW (float4 position) and a D3DCOLOR diffuse")]
    UnsupportedVertexFormat,
    /// The draw names a shader that was never registered.
    #[error("unknown shader {0:?}")]
    UnknownShader(ShaderId),
    /// `streams.len()` differs from the layout's stream count.
    #[error("draw supplies {given} vertex streams but the layout declares {declared}")]
    StreamCount {
        /// Streams supplied.
        given: usize,
        /// Streams declared.
        declared: usize,
    },
    /// Vertex data could not be repacked.
    #[error(transparent)]
    Repack(#[from] RepackError),
}

/// Which attachments a pass clears (D3D `Clear` flags, typed).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Clear {
    /// Clear colour (`D3DCLEAR_TARGET`).
    pub color: Option<[f32; 4]>,
    /// Clear depth (`D3DCLEAR_ZBUFFER`).
    pub depth: Option<f32>,
    /// Clear stencil (`D3DCLEAR_STENCIL`).
    pub stencil: Option<u8>,
}

impl Clear {
    /// Clear nothing.
    pub const NONE: Self = Self { color: None, depth: None, stencil: None };

    /// Clear everything: `color`, depth 1.0, stencil 0.
    pub fn all(color: [f32; 4]) -> Self {
        Self { color: Some(color), depth: Some(1.0), stencil: Some(0) }
    }
}

/// One guest vertex stream bound for a draw.
#[derive(Clone, Copy, Debug)]
pub struct VertexStream<'a> {
    /// Guest bytes, starting at the first vertex to draw.
    pub data: &'a [u8],
    /// Guest stride (`SetStreamSource`); `0` uses the layout's declared one.
    pub stride: u32,
}

/// One draw call.
pub struct Draw<'a> {
    /// Shader program ([`SHADER_XYZRHW_DIFFUSE`] or a registered one).
    pub shader: ShaderId,
    /// One entry per stream of `layout`, in the layout's order.
    pub streams: &'a [VertexStream<'a>],
    /// Number of vertices to draw.
    pub vertex_count: u32,
    /// Vertex format.
    pub layout: &'a VertexLayout,
    /// Primitive type, including Xbox-only ones.
    pub primitive: PrimitiveType,
    /// Render state in effect.
    pub state: RenderState,
    /// Extra bind groups for registered shaders, `(group index, group)`. The
    /// built-in shader manages group 0 itself.
    pub bind_groups: &'a [(u32, &'a wgpu::BindGroup)],
}

/// Draws guest primitives through the state model.
pub struct Renderer {
    shaders: HashMap<ShaderId, ShaderProgram>,
    viewport_buffer: wgpu::Buffer,
    viewport_bind_group: wgpu::BindGroup,
    cache: PipelineCache<wgpu::RenderPipeline>,
    caps: Capabilities,
    z_max: f32,
}

/// `D3DZ_MAX` for a 24-bit depth buffer: the range of pre-transformed z.
pub const D3DZ_MAX_24: f32 = 16_777_215.0;

impl Renderer {
    /// Creates the renderer and its built-in shader.
    pub fn new(ctx: &GfxContext) -> Self {
        let module = ctx.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("xyzrhw_diffuse"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/xyzrhw_diffuse.wgsl").into()),
        });
        let bgl = ctx.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewport"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let layout = ctx.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("xyzrhw layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let viewport_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viewport uniform"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let viewport_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewport"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: viewport_buffer.as_entire_binding() }],
        });
        let mut shaders = HashMap::new();
        shaders.insert(
            SHADER_XYZRHW_DIFFUSE,
            ShaderProgram { module, layout, vs_entry: "vs_main".into(), fs_entry: "fs_main".into() },
        );
        Self { shaders, viewport_buffer, viewport_bind_group, cache: PipelineCache::new(), caps: ctx.caps, z_max: D3DZ_MAX_24 }
    }

    /// Sets the range of pre-transformed (`XYZRHW`) z values, normally
    /// `D3DZ_MAX` of the depth format (default 16777215; 65535 for D16).
    pub fn set_pretransformed_z_max(&mut self, z_max: f32) {
        self.z_max = z_max.max(1.0);
    }

    /// Registers a generated shader under `id`. Returns `false` (and does
    /// nothing) for the built-in id. Re-registering an id replaces the
    /// program and drops all cached pipelines.
    pub fn register_shader(&mut self, id: ShaderId, program: ShaderProgram) -> bool {
        if id == SHADER_XYZRHW_DIFFUSE {
            return false;
        }
        if self.shaders.insert(id, program).is_some() {
            self.cache.clear();
        }
        true
    }

    /// The pipeline cache (for stats).
    pub fn cache(&self) -> &PipelineCache<wgpu::RenderPipeline> {
        &self.cache
    }

    fn builtin_supports(layout: &VertexLayout) -> bool {
        let has = |reg: VertexRegister, ty: VertexDataType| {
            layout.streams.iter().flat_map(|s| &s.attributes).any(|a| a.register == reg && a.data_type == ty)
        };
        layout.pretransformed
            && has(VertexRegister::POSITION, VertexDataType::Float4)
            && has(VertexRegister::DIFFUSE, VertexDataType::D3dColor)
    }

    /// Records one render pass containing `draws`, applying `clear` first.
    pub fn render(
        &mut self,
        ctx: &GfxContext,
        encoder: &mut wgpu::CommandEncoder,
        target: &RenderTarget<'_>,
        clear: Clear,
        draws: &[Draw<'_>],
    ) -> Result<(), DrawError> {
        // Validate everything and create GPU resources before the pass opens.
        struct Prepared<'a> {
            key: PipelineKey,
            shader: ShaderId,
            vertex_buffers: Vec<wgpu::Buffer>,
            indices: Option<(wgpu::Buffer, u32)>,
            vertex_count: u32,
            blend_constant: wgpu::Color,
            stencil_ref: u32,
            bind_groups: &'a [(u32, &'a wgpu::BindGroup)],
        }
        let formats = TargetFormats { color: target.color_format, depth: target.depth_format, samples: target.samples };
        let mut prepared = Vec::with_capacity(draws.len());
        for d in draws {
            if !self.shaders.contains_key(&d.shader) {
                return Err(DrawError::UnknownShader(d.shader));
            }
            if d.shader == SHADER_XYZRHW_DIFFUSE && !Self::builtin_supports(d.layout) {
                return Err(DrawError::UnsupportedVertexFormat);
            }
            if d.streams.len() != d.layout.streams.len() {
                return Err(DrawError::StreamCount { given: d.streams.len(), declared: d.layout.streams.len() });
            }
            let strides: Vec<u32> = d.streams.iter().map(|s| s.stride).collect();
            let plan = RepackPlan::new(d.layout, &strides);
            let mut vertex_buffers = Vec::with_capacity(d.streams.len());
            for (sp, vs) in plan.streams.iter().zip(d.streams) {
                let bytes = sp.repack(vs.data, d.vertex_count)?;
                // wgpu rejects zero-sized buffers; a 4-byte dummy keeps empty draws valid.
                let contents: &[u8] = if bytes.is_empty() { &[0; 4] } else { &bytes };
                vertex_buffers.push(ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("draw vertices"),
                    contents,
                    usage: wgpu::BufferUsages::VERTEX,
                }));
            }
            let key = PipelineKey::new(d.shader, d.layout, &strides, d.primitive, &d.state, formats, self.caps);
            let indices = match d.primitive.to_host(d.vertex_count) {
                HostPrimitive::Native(_) => None,
                HostPrimitive::Indexed(_, idx) => {
                    let n = idx.len() as u32;
                    let bytes: Vec<u8> = idx.iter().flat_map(|i| i.to_le_bytes()).collect();
                    let buf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("draw indices"),
                        contents: if bytes.is_empty() { &[0; 4] } else { &bytes },
                        usage: wgpu::BufferUsages::INDEX,
                    });
                    Some((buf, n))
                }
            };
            prepared.push(Prepared {
                key,
                shader: d.shader,
                vertex_buffers,
                indices,
                vertex_count: d.vertex_count,
                blend_constant: translate::blend_constant(&d.state),
                stencil_ref: d.state.stencil.reference as u32,
                bind_groups: d.bind_groups,
            });
        }
        for p in &prepared {
            let program = &self.shaders[&p.shader];
            self.cache.get_or_insert_with(&p.key, |k| create_render_pipeline(&ctx.device, k, program, "xbox draw"));
        }

        let (w, h) = target.size;
        ctx.queue.write_buffer(&self.viewport_buffer, 0, bytemuck::cast_slice(&[w as f32, h as f32, 1.0 / self.z_max, 0.0]));

        let color_load = match clear.color {
            Some([r, g, b, a]) => wgpu::LoadOp::Clear(wgpu::Color { r: r as f64, g: g as f64, b: b as f64, a: a as f64 }),
            None => wgpu::LoadOp::Load,
        };
        let has_stencil = target.depth_format.is_some_and(|f| f.has_stencil_aspect());
        let depth_attachment = target.depth.map(|view| wgpu::RenderPassDepthStencilAttachment {
            view,
            depth_ops: Some(wgpu::Operations {
                load: clear.depth.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: has_stencil.then_some(wgpu::Operations {
                load: clear.stencil.map_or(wgpu::LoadOp::Load, |s| wgpu::LoadOp::Clear(s as u32)),
                store: wgpu::StoreOp::Store,
            }),
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gfx pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target.color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: color_load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: depth_attachment,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        // The attachments may be larger than the logical target (a depth
        // surface bigger than the colour surface): confine drawing to it.
        pass.set_viewport(0.0, 0.0, w as f32, h as f32, 0.0, 1.0);
        pass.set_scissor_rect(0, 0, w, h);
        for p in &prepared {
            pass.set_pipeline(self.cache.get(&p.key).expect("built above"));
            if p.shader == SHADER_XYZRHW_DIFFUSE {
                pass.set_bind_group(0, &self.viewport_bind_group, &[]);
            }
            for (index, group) in p.bind_groups {
                pass.set_bind_group(*index, *group, &[]);
            }
            pass.set_blend_constant(p.blend_constant);
            pass.set_stencil_reference(p.stencil_ref);
            for (i, buf) in p.vertex_buffers.iter().enumerate() {
                pass.set_vertex_buffer(i as u32, buf.slice(..));
            }
            match &p.indices {
                None => pass.draw(0..p.vertex_count, 0..1),
                Some((buf, n)) => {
                    pass.set_index_buffer(buf.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..*n, 0, 0..1);
                }
            }
        }
        Ok(())
    }
}
