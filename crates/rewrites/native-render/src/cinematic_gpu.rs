//! The Halo 1 cinematic screen pass: blur, light, desaturation, tint and the
//! video effect over a colour target (docs/graphics/native-post-hud.md).
//! The host (the game's `native_cinematic_glue`) supplies the target and the
//! parameters; nothing here reads guest memory or names the device.
use crate::gpu::Image;
use std::collections::HashMap;
use wgpu::util::DeviceExt;

/// One frame's controls, read from the campaign's globals by the host.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub passes: u16,
    pub kernel: i16,
    pub radius: f32,
    pub light: f32,
    pub desat: f32,
    pub tint: [f32; 3],
    pub filtered: bool,
    pub video: bool,
    pub gain: f32,
    pub noise: f32,
    /// The video noise's UV offset for this tick.
    pub shift: [f32; 2],
}

/// What the pass draws into.
pub struct Target<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub color: &'a wgpu::TextureView,
    pub color_format: wgpu::TextureFormat,
    /// The viewport `(x, y, width, height)` in the target's texels.
    pub viewport: [f32; 4],
    /// The target's size in guest pixels and in this copy's texels.
    pub guest_size: (u32, u32),
    pub size: (u32, u32),
    /// The display copy, against the render copy: each has its own scratch.
    pub display: bool,
}

/// The video effect's bitmaps by map: a mask and a noise.
pub struct Video<'a> {
    pub map: &'a str,
    pub images: &'a dyn Fn() -> Option<[Image; 2]>,
}

#[derive(Default)]
pub struct Cache {
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    sampler: Option<wgpu::Sampler>,
    copies: [Option<[wgpu::TextureView; 2]>; 2],
    video: Option<(String, [wgpu::TextureView; 2])>,
}

fn pipeline(device: &wgpu::Device, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Halo 1 cinematic screen effect"),
        source: wgpu::ShaderSource::Wgsl(include_str!("cinematic.wgsl").into()),
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Halo 1 cinematic screen effect"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn upload(t: &Target<'_>, image: &Image) -> wgpu::TextureView {
    let texture = t.device.create_texture_with_data(
        t.queue,
        &wgpu::TextureDescriptor {
            label: Some("Halo 1 video effect bitmap"),
            size: wgpu::Extent3d {
                width: image.width,
                height: image.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &image.rgba,
    );
    texture.create_view(&Default::default())
}

impl Cache {
    /// Encodes the pass over `t`. `false`: the video bitmaps were missing and
    /// nothing was drawn.
    pub fn encode(&mut self, t: Target<'_>, f: Params, video: Option<Video<'_>>) -> bool {
        let cache = self;
        let color = t.color.texture().clone();
        let extent = color.size();
        let format = t.color_format;
        let slot = &mut cache.copies[usize::from(t.display)];
        if slot.as_ref().is_none_or(|views| {
            views[0].texture().size() != extent || views[0].texture().format() != format
        }) {
            *slot = Some(std::array::from_fn(|_| {
                t.device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("Halo 1 cinematic convolution"),
                        size: extent,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::COPY_DST
                            | wgpu::TextureUsages::TEXTURE_BINDING
                            | wgpu::TextureUsages::RENDER_ATTACHMENT,
                        view_formats: &[],
                    })
                    .create_view(&Default::default())
            }));
        }
        let scratch = slot.as_ref().expect("allocated above").clone();
        let pipe = cache
            .pipelines
            .entry(format)
            .or_insert_with(|| pipeline(t.device, format))
            .clone();
        let sampler = cache
            .sampler
            .get_or_insert_with(|| {
                t.device.create_sampler(&wgpu::SamplerDescriptor {
                    min_filter: wgpu::FilterMode::Linear,
                    mag_filter: wgpu::FilterMode::Linear,
                    ..Default::default()
                })
            })
            .clone();
        let video = if let Some(v) = video {
            if cache.video.as_ref().is_none_or(|(key, _)| key != v.map) {
                let Some(images) = (v.images)() else {
                    return false;
                };
                cache.video = Some((v.map.to_owned(), images.map(|im| upload(&t, &im))));
            }
            Some(cache.video.as_ref().unwrap().1.clone())
        } else {
            None
        };
        let scale = [
            t.size.0 as f32 / t.guest_size.0 as f32,
            t.size.1 as f32 / t.guest_size.1 as f32,
        ];
        t.encoder.copy_texture_to_texture(
            color.as_image_copy(),
            scratch[0].texture().as_image_copy(),
            extent,
        );
        let shift = if f.video { f.shift } else { [0.; 2] };
        let count = if f.kernel != 0 && f.radius > 0. {
            // The pass loop compares the low signed word at H1 0x6c4dd.
            (f.passes as i16).wrapping_add(1).wrapping_mul(2).max(0) as usize
        } else {
            1
        };
        for stage in 0..count {
            let source = &scratch[stage % 2];
            let last = stage + 1 == count;
            let dest = if last {
                t.color
            } else {
                &scratch[(stage + 1) % 2]
            };
            let [x, y, width, height] = t.viewport;
            let control = [
                x,
                y,
                width,
                height,
                scale[0] * f.radius,
                scale[1] * f.radius,
                if count > 1 { f32::from(f.kernel) } else { 0. },
                0.,
                f.light,
                f.desat,
                f.filtered as u8 as f32,
                last as u8 as f32,
                f.tint[0],
                f.tint[1],
                f.tint[2],
                0.,
                f.video as u8 as f32,
                f.gain,
                f.noise,
                0.,
                scale[0],
                scale[1],
                shift[0],
                shift[1],
            ];
            let bytes = control.map(f32::to_le_bytes);
            let buffer = t
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("cinematic screen controls"),
                    contents: bytes.as_flattened(),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
            let group = t.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("cinematic screen inputs"),
                layout: &pipe.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(source),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(
                            video.as_ref().map_or(source, |v| &v[0]),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(
                            video.as_ref().map_or(source, |v| &v[1]),
                        ),
                    },
                ],
            });
            let mut pass = t.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cinematic screen pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: dest,
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
            pass.set_viewport(x, y, width, height, 0., 1.);
            pass.set_pipeline(&pipe);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn cinematic_shader_validates() {
        let module = wgpu::naga::front::wgsl::parse_str(include_str!("cinematic.wgsl")).unwrap();
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap();
    }
}
