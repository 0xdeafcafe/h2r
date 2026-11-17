//! Partial and channel-masked clears (`D3DDevice_Clear` with rectangles, or
//! with only some of `D3DCLEAR_TARGET_R/G/B/A`).
//!
//! Load operations only clear whole attachments. Anything smaller is drawn:
//! a full-screen triangle per rectangle, scissored to it, whose pipeline
//! writes the constant colour through the channel mask, replaces depth with
//! the clear value (compare always) and replaces stencil (compare always,
//! write mask `0xFF`). A request that covers the whole target with every
//! colour channel falls back to load operations.

use std::collections::HashMap;

use wgpu::util::DeviceExt;

use crate::context::GfxContext;
use crate::presenter::RenderTarget;
use crate::renderer::ScissorRect;

/// What [`crate::Renderer::clear_rects`] clears.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaskedClear {
    /// Colour value, RGBA in `0..=1`; `None` leaves colour alone.
    pub color: Option<[f32; 4]>,
    /// Colour channels written (ignored when `color` is `None`).
    pub channels: wgpu::ColorWrites,
    /// Depth value; ignored without a depth attachment.
    pub depth: Option<f32>,
    /// Stencil value; ignored without a stencil aspect.
    pub stencil: Option<u8>,
}

impl MaskedClear {
    /// All colour channels to `color`, nothing else.
    pub fn color(color: [f32; 4]) -> Self {
        Self {
            color: Some(color),
            channels: wgpu::ColorWrites::ALL,
            depth: None,
            stencil: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Key {
    color: wgpu::TextureFormat,
    depth: Option<wgpu::TextureFormat>,
    samples: u32,
    writes: wgpu::ColorWrites,
    depth_write: bool,
    stencil_write: bool,
}

const SHADER: &str = r#"
struct ClearValue { color: vec4f, depth: vec4f, }
@group(0) @binding(0) var<uniform> value: ClearValue;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    return vec4f(x, y, value.depth.x, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4f {
    return value.color;
}
"#;

/// Pipelines for drawn clears.
pub(crate) struct Clearer {
    module: wgpu::ShaderModule,
    bgl: wgpu::BindGroupLayout,
    layout: wgpu::PipelineLayout,
    pipelines: HashMap<Key, wgpu::RenderPipeline>,
}

impl Clearer {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("masked clear"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("masked clear"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("masked clear"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        Self {
            module,
            bgl,
            layout,
            pipelines: HashMap::new(),
        }
    }

    fn pipeline(&mut self, device: &wgpu::Device, k: Key) -> &wgpu::RenderPipeline {
        let (module, layout) = (&self.module, &self.layout);
        self.pipelines.entry(k).or_insert_with(|| {
            let stencil = if k.stencil_write {
                let face = wgpu::StencilFaceState {
                    compare: wgpu::CompareFunction::Always,
                    fail_op: wgpu::StencilOperation::Replace,
                    depth_fail_op: wgpu::StencilOperation::Replace,
                    pass_op: wgpu::StencilOperation::Replace,
                };
                wgpu::StencilState {
                    front: face,
                    back: face,
                    read_mask: 0xFF,
                    write_mask: 0xFF,
                }
            } else {
                wgpu::StencilState::default()
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("masked clear"),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: k.depth.map(|format| wgpu::DepthStencilState {
                    format,
                    depth_write_enabled: Some(k.depth_write),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil,
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: k.samples,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: k.color,
                        blend: None,
                        write_mask: k.writes,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        })
    }

    pub(crate) fn clear(
        &mut self,
        ctx: &GfxContext,
        encoder: &mut wgpu::CommandEncoder,
        target: &RenderTarget<'_>,
        clear: MaskedClear,
        rects: &[ScissorRect],
    ) {
        let (w, h) = target.size;
        let full = ScissorRect::full(w, h);
        let rects: Vec<ScissorRect> = rects
            .iter()
            .map(|r| r.intersect(full))
            .filter(|r| !r.is_empty())
            .collect();
        let writes = if clear.color.is_some() {
            clear.channels
        } else {
            wgpu::ColorWrites::empty()
        };
        let depth = clear.depth.filter(|_| target.depth.is_some());
        let has_stencil =
            target.depth.is_some() && target.depth_format.is_some_and(|f| f.has_stencil_aspect());
        let stencil = clear.stencil.filter(|_| has_stencil);
        if rects.is_empty() || (writes.is_empty() && depth.is_none() && stencil.is_none()) {
            return;
        }
        let color = clear.color.unwrap_or([0.0; 4]);
        let whole = rects.iter().any(|r| r.covers(w, h));
        if whole && (writes.is_empty() || writes == wgpu::ColorWrites::ALL) {
            let load = |on: bool| {
                if on {
                    wgpu::LoadOp::Clear(wgpu::Color {
                        r: color[0] as f64,
                        g: color[1] as f64,
                        b: color[2] as f64,
                        a: color[3] as f64,
                    })
                } else {
                    wgpu::LoadOp::Load
                }
            };
            let _pass = begin(
                encoder,
                target,
                load(!writes.is_empty()),
                depth,
                stencil,
                has_stencil,
            );
            return;
        }
        let value: Vec<u8> = color
            .iter()
            .chain(&[depth.unwrap_or(0.0), 0.0, 0.0, 0.0])
            .flat_map(|f| f.to_le_bytes())
            .collect();
        let buffer = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("masked clear value"),
                contents: &value,
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("masked clear"),
            layout: &self.bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        let key = Key {
            color: target.color_format,
            depth: target.depth.and(target.depth_format),
            samples: target.samples,
            writes,
            depth_write: depth.is_some(),
            stencil_write: stencil.is_some(),
        };
        let pipeline = self.pipeline(&ctx.device, key);
        let mut pass = begin(encoder, target, wgpu::LoadOp::Load, None, None, has_stencil);
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.set_stencil_reference(stencil.unwrap_or(0) as u32);
        for r in rects {
            pass.set_scissor_rect(r.x, r.y, r.width, r.height);
            pass.draw(0..3, 0..1);
        }
    }
}

fn begin<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    target: &RenderTarget<'_>,
    color: wgpu::LoadOp<wgpu::Color>,
    depth: Option<f32>,
    stencil: Option<u8>,
    has_stencil: bool,
) -> wgpu::RenderPass<'e> {
    crate::churn::pass();
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("gfx clear"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target.color,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: color,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: target
            .depth
            .map(|view| wgpu::RenderPassDepthStencilAttachment {
                view,
                depth_ops: Some(wgpu::Operations {
                    load: depth.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: has_stencil.then_some(wgpu::Operations {
                    load: stencil.map_or(wgpu::LoadOp::Load, |s| wgpu::LoadOp::Clear(s as u32)),
                    store: wgpu::StoreOp::Store,
                }),
            }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}
