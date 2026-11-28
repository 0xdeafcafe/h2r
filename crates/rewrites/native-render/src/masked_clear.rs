//! A clear of some of a colour target's channels (D3D `Clear` with
//! `D3DCLEAR_TARGET_R`, `_G`, `_B`, `_A`, `0x10` to `0x80`, not all four),
//! which a render pass's load cannot make: one triangle over the whole
//! bound colour whose blend writes the blend constant into the named
//! channels only. The ocean's alpha clear (`0x0003c515`, D3D `0x80`) is
//! one.

use std::sync::Mutex;

use gfx::external::ExternalAttachments;

use crate::program::ViewInputs;
use crate::stage::{Stage, StageWork};

/// The channels of D3D clear flags `flags` (`0x10` red to `0x80` alpha).
pub fn channels(flags: u32) -> wgpu::ColorWrites {
    let mut out = wgpu::ColorWrites::empty();
    for (bit, c) in [(0x10, wgpu::ColorWrites::RED), (0x20, wgpu::ColorWrites::GREEN), (0x40, wgpu::ColorWrites::BLUE), (0x80, wgpu::ColorWrites::ALPHA)] {
        if flags & bit != 0 {
            out |= c;
        }
    }
    out
}

/// The clear as a stage on the bound colour.
pub struct MaskedClear {
    pub channels: wgpu::ColorWrites,
    pub colour: [f32; 4],
}

const WGSL: &str = "
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}
@fragment fn fs() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0);
}
";

/// Each device's pipelines, by colour format and channels.
type Pipelines = Vec<(wgpu::Device, wgpu::TextureFormat, wgpu::ColorWrites, wgpu::RenderPipeline)>;
static PIPELINES: Mutex<Pipelines> = Mutex::new(Vec::new());

fn pipeline(device: &wgpu::Device, format: wgpu::TextureFormat, channels: wgpu::ColorWrites) -> wgpu::RenderPipeline {
    let mut held = PIPELINES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((.., p)) = held.iter().find(|(d, f, c, _)| d == device && *f == format && *c == channels) {
        return p.clone();
    }
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("masked clear"),
        source: wgpu::ShaderSource::Wgsl(WGSL.into()),
    });
    // The constant times one, the target's texel times zero.
    let constant = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Constant,
        dst_factor: wgpu::BlendFactor::Zero,
        operation: wgpu::BlendOperation::Add,
    };
    let p = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("masked clear"),
        layout: None,
        vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState { color: constant, alpha: constant }),
                write_mask: channels,
            })],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    });
    held.push((device.clone(), format, channels, p.clone()));
    p
}

impl Stage for MaskedClear {
    fn name(&self) -> &'static str {
        "masked clear"
    }

    fn record(&mut self, t: &ExternalAttachments, _: &ViewInputs) -> StageWork {
        let pipeline = pipeline(&t.device, t.color_format, self.channels);
        let (color, [r, g, b, a]) = (t.color.clone(), self.colour);
        let constant = wgpu::Color { r: r.into(), g: g.into(), b: b.into(), a: a.into() };
        StageWork::Encoded(gfx::external::ExternalEncoded {
            ranges: 1,
            draws: 1,
            passes: 1,
            commands: Box::new(move |encoder| {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("masked clear"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &color,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    })],
                    ..Default::default()
                });
                pass.set_pipeline(&pipeline);
                pass.set_blend_constant(constant);
                pass.draw(0..3, 0..1);
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_d3d_channel_bits_name_their_channels() {
        assert_eq!(channels(0x80), wgpu::ColorWrites::ALPHA);
        assert_eq!(channels(0x30), wgpu::ColorWrites::RED | wgpu::ColorWrites::GREEN);
        assert_eq!(channels(0xf0), wgpu::ColorWrites::ALL);
        assert_eq!(channels(0x03), wgpu::ColorWrites::empty());
    }
}
