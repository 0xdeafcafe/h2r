//! A shader-driven gel producer. Each invocation owns its texture; later
//! lights cannot overwrite a retained result. The light graph binds
//! `Output::view` in the existing `LightStage::Gel` slot.

use std::sync::{Arc, Mutex};

use gfx::translate::SamplerKey;
use shaders::psh::{self, PixelOptions, PixelShaderDef, PixelUniforms, TextureDim};
use shaders::vsh::{self, VertexOptions, VertexUniforms};
use wgpu::util::DeviceExt;

use crate::gels::{self, Key};
use crate::gpu::{Image, texture};
use crate::pass_state::PassState;

pub const INPUTS: u16 = 0x7f;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub registers: [[f32; 4]; 7],
}

/// `0x00015ec0`'s four vertices. It pushes v1..v4 with 2F (z=0,w=1),
/// v5/v6 with 4F, then v0, in triangle-strip order.
pub fn quad(size: (u32, u32), depth: [f32; 2]) -> [Vertex; 4] {
    std::array::from_fn(|n| {
        let u = (n & 1) as f32;
        let v = (n >> 1) as f32;
        let mut registers = [[0.0; 4]; 7];
        registers[0] = [u * size.0 as f32, v * size.1 as f32, depth[0], depth[1]];
        registers[1..5].fill([u, v, 0.0, 1.0]);
        Vertex { registers }
    })
}

pub struct Program {
    pub wgsl: String,
    pub dims: [Option<TextureDim>; 4],
    pub samplers: [SamplerKey; 4],
    pub state: PassState,
}

impl Program {
    /// The host supplies the evaluated pixel definition and sampler/state
    /// snapshot, after guest shader setup. Constant animations therefore
    /// use the same values as D3D without another function evaluator.
    pub fn compile(
        microcode: &[u8],
        def: &PixelShaderDef,
        options: &PixelOptions,
        samplers: [SamplerKey; 4],
        state: PassState,
    ) -> Result<Self, String> {
        let program =
            vsh::Program::parse(microcode).map_err(|e| format!("gel vertex program: {e:?}"))?;
        if program.inputs_read() & !INPUTS != 0 {
            return Err("gel reads inherited vertex attributes".into());
        }
        let vs = vsh::translate(
            &program,
            &VertexOptions {
                provided_inputs: INPUTS,
                ..Default::default()
            },
        )
        .map_err(|e| format!("gel vertex translation: {e:?}"))?;
        let ps = psh::translate(def, options);
        if ps.discards {
            return Err("gel can leave discarded pixels from the previous target".into());
        }
        if !ps.warnings.is_empty() {
            return Err(format!("gel pixel translation: {:?}", ps.warnings));
        }
        if state.depth_write
            || state.depth_compare != wgpu::CompareFunction::Always
            || options.w_buffer
        {
            return Err("gel requires depth".into());
        }
        // A fresh target must not depend on old target-13 pixels.
        if state.blend.is_some() || state.color_writes != wgpu::ColorWrites::ALL {
            return Err("gel depends on previous target contents".into());
        }
        let wgsl = shaders::link(&vs, &ps).map_err(|e| format!("gel link: {e}"))?;
        Ok(Self {
            wgsl,
            dims: ps.textures,
            samplers,
            state,
        })
    }
}

/// Per-production uniform data. Buffers are allocated for the submission,
/// so reference/scaled encodes never overwrite each other's uniforms.
pub struct Frame {
    pub vertex: VertexUniforms,
    pub pixel: PixelUniforms,
    pub vertices: [Vertex; 4],
}

/// A fresh output is valid only when all four transformed corners cover
/// the full target and survive clipping. Otherwise untouched pixels need
/// the previous contents of target 13, which this producer does not own.
pub fn full_target(microcode: &[u8], frame: &Frame, size: (u32, u32)) -> Result<(), String> {
    let program = vsh::Program::parse(microcode).map_err(|e| format!("gel coverage: {e:?}"))?;
    for (n, vertex) in frame.vertices.iter().enumerate() {
        let mut inputs = frame.vertex.defaults;
        inputs[..7].copy_from_slice(&vertex.registers);
        let mut constants = frame.vertex.constants;
        let pos = vsh::interp::run(&program, &inputs, &mut constants).regs[0];
        if pos.iter().any(|v| !v.is_finite()) || pos[3] <= 0.0 {
            return Err("gel has an unestablished full-target projection".into());
        }
        let surface = [size.0 as f32, size.1 as f32];
        let clip = match frame.vertex.w_clip {
            Some(range) => vsh::interp::fixup_w_buffer(pos, surface, range, frame.vertex.w_clamp),
            None => vsh::interp::fixup(pos, surface, frame.vertex.z_max),
        };
        let expected = [
            if n & 1 == 0 { -1.0 } else { 1.0 },
            if n & 2 == 0 { 1.0 } else { -1.0 },
        ];
        if clip.iter().any(|v| !v.is_finite())
            || clip[0] / clip[3] != expected[0]
            || clip[1] / clip[3] != expected[1]
            || clip[2] < 0.0
            || clip[2] > clip[3]
        {
            return Err("gel does not replace every target pixel".into());
        }
    }
    Ok(())
}

#[derive(Clone)]
pub struct Output {
    pub key: Key,
    pub view: wgpu::TextureView,
    pub guest_size: (u32, u32),
    pub size: (u32, u32),
}

/// A ticket is retained for a particular light invocation. Publish only
/// after all commands for a copy were recorded. A missing copy refuses
/// the consumer; it must never borrow the previous light's output.
#[derive(Clone)]
pub struct Ticket {
    pub key: Key,
    copies: Arc<Mutex<Vec<Output>>>,
}

impl Ticket {
    pub fn new(key: Key) -> Self {
        Self {
            key,
            copies: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn publish(&self, output: Output) {
        if output.key != self.key {
            return;
        }
        let mut copies = self.copies.lock().unwrap_or_else(|e| e.into_inner());
        copies.retain(|c| (c.guest_size, c.size) != (output.guest_size, output.size));
        copies.push(output);
    }

    pub fn output(&self, key: Key, guest_size: (u32, u32), size: (u32, u32)) -> Option<Output> {
        if key != self.key {
            return None;
        }
        self.copies
            .lock()
            .ok()?
            .iter()
            .find(|c| c.guest_size == guest_size && c.size == size)
            .cloned()
    }
}

pub struct Producer {
    program: Program,
    textures: wgpu::BindGroup,
    uniforms_layout: wgpu::BindGroupLayout,
    layout: wgpu::PipelineLayout,
    module: wgpu::ShaderModule,
}

impl Producer {
    /// Resolve every sampled image before recording anything. No default
    /// texture or partial gel is published on failure.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        program: Program,
        images: &[Option<Image>; 4],
    ) -> Result<Self, String> {
        let mut views = Vec::new();
        let mut samplers = Vec::new();
        let mut entries = Vec::new();
        for n in 0..4 {
            let Some(dim) = program.dims[n] else { continue };
            if dim != TextureDim::D2 {
                return Err(format!("gel stage {n} is not 2D"));
            }
            let image = images[n]
                .as_ref()
                .ok_or_else(|| format!("gel stage {n} is unresolved"))?;
            views.push((n, texture(device, queue, image, "native gel bitmap")));
            samplers.push(device.create_sampler(&program.samplers[n].descriptor()));
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 2 * n as u32,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
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
        let textures_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native gel textures"),
            entries: &entries,
        });
        let mut bindings = Vec::new();
        for ((n, view), sampler) in views.iter().zip(&samplers) {
            bindings.push(wgpu::BindGroupEntry {
                binding: 2 * *n as u32,
                resource: wgpu::BindingResource::TextureView(view),
            });
            bindings.push(wgpu::BindGroupEntry {
                binding: 2 * *n as u32 + 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            });
        }
        let textures = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native gel textures"),
            layout: &textures_layout,
            entries: &bindings,
        });
        let uniforms_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native gel uniforms"),
            entries: &std::array::from_fn::<_, 2, _>(|n| wgpu::BindGroupLayoutEntry {
                binding: n as u32,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(if n == 0 {
                        VertexUniforms::SIZE
                    } else {
                        PixelUniforms::SIZE
                    } as u64),
                },
                count: None,
            }),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native gel"),
            bind_group_layouts: &[Some(&uniforms_layout), Some(&textures_layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native gel"),
            source: wgpu::ShaderSource::Wgsl(program.wgsl.clone().into()),
        });
        Ok(Self {
            program,
            textures,
            uniforms_layout,
            layout,
            module,
        })
    }

    /// Records the native equivalent of target 13. The caller invokes
    /// this at the original quad, before scheduling the consuming light.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        key: Key,
        frame: &Frame,
        format: wgpu::TextureFormat,
        guest_size: (u32, u32),
        size: (u32, u32),
    ) -> Result<Output, String> {
        if guest_size.0 == 0 || guest_size.1 == 0 || size.0 == 0 || size.1 == 0 {
            return Err("empty gel target".into());
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("native target 13"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let mut vu = frame.vertex.clone();
        vu.surface = [guest_size.0 as f32, guest_size.1 as f32];
        vu.shift = gfx::renderer::window_shift(guest_size.0, size.0);
        let mut vb = vec![0; VertexUniforms::SIZE];
        vu.write_bytes(&mut vb);
        let mut pb = vec![0; PixelUniforms::SIZE];
        frame.pixel.write_bytes(&mut pb);
        let uniform = |bytes: &[u8]| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("native gel uniforms"),
                contents: bytes,
                usage: wgpu::BufferUsages::UNIFORM,
            })
        };
        let vertex = uniform(&vb);
        let pixel = uniform(&pb);
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native gel uniforms"),
            layout: &self.uniforms_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: vertex.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: pixel.as_entire_binding(),
                },
            ],
        });
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("native gel quad"),
            contents: bytemuck::cast_slice(&frame.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let attributes: [wgpu::VertexAttribute; 7] =
            std::array::from_fn(|n| wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: n as u64 * 16,
                shader_location: n as u32,
            });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("native target 13"),
            layout: Some(&self.layout),
            vertex: wgpu::VertexState {
                module: &self.module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..self.program.state.primitive
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &self.module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native target 13"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.set_bind_group(1, &self.textures, &[]);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.draw(0..4, 0..1);
        drop(pass);
        Ok(Output {
            key,
            view,
            guest_size,
            size,
        })
    }
}

/// Exact byte compare of two same-format copies at the production point.
/// Mapping is scheduled on submission, never before its copy is submitted.
pub fn compare_on_submit(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    output: &Output,
    reference: &wgpu::TextureView,
) -> Result<(), String> {
    let a = output.view.texture();
    let b = reference.texture();
    if a.size() != b.size()
        || a.format() != b.format()
        || a.sample_count() != 1
        || b.sample_count() != 1
        || !b.usage().contains(wgpu::TextureUsages::COPY_SRC)
    {
        return Err("unpaired target-13 copies".into());
    }
    let bpp = a
        .format()
        .block_copy_size(None)
        .ok_or("gel format has no copy size")?;
    let row = a.width().checked_mul(bpp).ok_or("gel row overflow")?;
    let padded = row.div_ceil(256) * 256;
    let bytes = u64::from(padded) * u64::from(a.height());
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("native gels compare"),
        size: bytes * 2,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    for (texture, offset) in [(a, 0), (b, bytes)] {
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(a.height()),
                },
            },
            a.size(),
        );
    }
    let read = buffer.clone();
    let key = output.key;
    let size = output.size;
    encoder.map_buffer_on_submit(&buffer, wgpu::MapMode::Read, .., move |result| {
        if let Err(e) = result {
            eprintln!("native-gels compare {key:?} {size:?}: map failed {e:?}");
            return;
        }
        let result = read.slice(..).get_mapped_range().map(|data| {
            let pack = |data: &[u8]| {
                data.chunks_exact(padded as usize)
                    .flat_map(|r| r[..row as usize].iter().copied())
                    .collect::<Vec<_>>()
            };
            gels::compare(
                &pack(&data[..bytes as usize]),
                &pack(&data[bytes as usize..]),
            )
        });
        match result {
            Ok(Some(d)) => eprintln!(
                "native-gels compare {key:?} {size:?}: {} of {} bytes differ, max {}, sum {}",
                d.changed, d.bytes, d.max, d.sum
            ),
            _ => eprintln!("native-gels compare {key:?} {size:?}: unreadable pair"),
        }
        read.unmap();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quad_matches_the_helper_register_writes() {
        let q = quad((32, 16), [0.25, 0.5]);
        assert_eq!(q[0].registers[0], [0.0, 0.0, 0.25, 0.5]);
        assert_eq!(q[1].registers[0], [32.0, 0.0, 0.25, 0.5]);
        assert_eq!(q[2].registers[0], [0.0, 16.0, 0.25, 0.5]);
        assert_eq!(q[3].registers[0], [32.0, 16.0, 0.25, 0.5]);
        for v in q {
            assert_eq!(
                v.registers[1],
                [v.registers[0][0] / 32.0, v.registers[0][1] / 16.0, 0.0, 1.0]
            );
            assert_eq!(v.registers[5], [0.0; 4]);
            assert_eq!(v.registers[6], [0.0; 4]);
        }
    }

    #[test]
    fn tickets_refuse_another_invocation_and_missing_scaled_copies() {
        let key = Key {
            sequence: 1,
            definition: 2,
            shader: 3,
            surface: 4,
        };
        let ticket = Ticket::new(key);
        assert!(ticket.output(key, (32, 32), (32, 32)).is_none());
        assert!(
            ticket
                .output(Key { sequence: 2, ..key }, (32, 32), (32, 32))
                .is_none()
        );
        assert!(ticket.output(key, (32, 32), (64, 64)).is_none());
    }
}
