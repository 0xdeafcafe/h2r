//! A one-line notice drawn over the game frame (`h2 play --follow`'s
//! "Update ready" banner), dismissable, with an optional restart offer.
//!
//! [`Banner`] is a shared handle: any thread sets the notice, the window
//! draws it. Text is rasterised on the CPU with the public-domain `font8x8`
//! glyphs (ASCII; a few typographic characters are mapped to ASCII), scaled
//! 2x, on a translucent plate, and drawn in the top-left corner of the
//! window with alpha blending. F6 hides the current notice (a new notice
//! shows again); F5 asks for a restart while the notice offers one.

use std::sync::{Arc, Mutex, MutexGuard};

/// What a notice is about; picks its colour and whether F5 restarts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    /// Work in progress ("Building ...").
    Info,
    /// An update is ready: F5 restarts into it.
    Ready,
    /// Something failed; the game keeps running as it is.
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub kind: NoticeKind,
    pub text: String,
}

#[derive(Default)]
struct State {
    notice: Option<Notice>,
    dismissed: bool,
    /// Bumped on every visible change, so the window knows to redraw.
    generation: u64,
}

/// Shared handle to the window's notice. Cloning shares it.
#[derive(Clone, Default)]
pub struct Banner(Arc<Mutex<State>>);

impl std::fmt::Debug for Banner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Banner").field(&self.visible()).finish()
    }
}

impl Banner {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Show `notice` (or clear with `None`). A different notice than the
    /// current one is shown even if the current one was dismissed.
    pub fn set(&self, notice: Option<Notice>) {
        let mut s = self.lock();
        if s.notice != notice {
            s.notice = notice;
            s.dismissed = false;
            s.generation += 1;
        }
    }

    /// Hide the current notice until a different one is set.
    pub fn dismiss(&self) {
        let mut s = self.lock();
        if !s.dismissed && s.notice.is_some() {
            s.dismissed = true;
            s.generation += 1;
        }
    }

    /// The notice to draw, if any.
    pub fn visible(&self) -> Option<Notice> {
        let s = self.lock();
        s.notice.clone().filter(|_| !s.dismissed)
    }

    /// The current notice offers a restart (dismissed or not: F5 still
    /// works after hiding the banner).
    pub fn restart_offered(&self) -> bool {
        self.lock()
            .notice
            .as_ref()
            .is_some_and(|n| n.kind == NoticeKind::Ready)
    }

    pub fn generation(&self) -> u64 {
        self.lock().generation
    }
}

/// Glyph scale and padding, in window pixels.
const SCALE: u32 = 2;
const PAD: u32 = 6;
/// Longest line drawn; the rest is cut with "...".
const MAX_CHARS: usize = 96;

/// ASCII stand-ins for the characters notices commonly use.
fn ascii(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '\u{2014}' | '\u{2013}' => out.push('-'),
            '\u{2026}' => out.push_str("..."),
            '\u{2018}' | '\u{2019}' => out.push('\''),
            '\u{201c}' | '\u{201d}' => out.push('"'),
            c if c.is_ascii() && !c.is_ascii_control() => out.push(c),
            _ => out.push('?'),
        }
    }
    if out.chars().count() > MAX_CHARS {
        out = out.chars().take(MAX_CHARS - 3).collect::<String>() + "...";
    }
    out
}

/// The notice as straight-alpha RGBA: (width, height, pixels).
pub fn rasterize(notice: &Notice) -> (u32, u32, Vec<u8>) {
    use font8x8::UnicodeFonts;
    let text = ascii(&notice.text);
    let n = text.chars().count().max(1) as u32;
    let (w, h) = (n * 8 * SCALE + 2 * PAD, 8 * SCALE + 2 * PAD);
    let plate: [u8; 4] = match notice.kind {
        NoticeKind::Info => [24, 32, 48, 210],
        NoticeKind::Ready => [16, 72, 32, 225],
        NoticeKind::Error => [96, 20, 20, 225],
    };
    let mut px: Vec<u8> = plate
        .iter()
        .copied()
        .cycle()
        .take((w * h * 4) as usize)
        .collect();
    for (i, c) in text.chars().enumerate() {
        let Some(glyph) = font8x8::BASIC_FONTS.get(c) else {
            continue;
        };
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..8u32 {
                if bits & (1 << col) == 0 {
                    continue;
                }
                for dy in 0..SCALE {
                    for dx in 0..SCALE {
                        let x = PAD + (i as u32 * 8 + col) * SCALE + dx;
                        let y = PAD + row as u32 * SCALE + dy;
                        let o = ((y * w + x) * 4) as usize;
                        px[o..o + 4].copy_from_slice(&[240, 240, 240, 255]);
                    }
                }
            }
        }
    }
    (w, h, px)
}

const OVERLAY_WGSL: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    var out: VsOut;
    out.clip = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.uv = vec2<f32>(x, y);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(src, smp, in.uv);
}
"#;

/// Draws the banner's notice over an already composited target.
pub struct OverlayRenderer {
    module: wgpu::ShaderModule,
    bgl: wgpu::BindGroupLayout,
    layout: wgpu::PipelineLayout,
    sampler: wgpu::Sampler,
    pipelines: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    /// The last notice rasterised, its bind group and size.
    cached: Option<(Notice, wgpu::BindGroup, (u32, u32))>,
}

impl OverlayRenderer {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("app overlay"),
            source: wgpu::ShaderSource::Wgsl(OVERLAY_WGSL.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("app overlay"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("app overlay"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("app overlay"),
            ..Default::default()
        });
        Self {
            module,
            bgl,
            layout,
            sampler,
            pipelines: Vec::new(),
            cached: None,
        }
    }

    fn pipeline(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> wgpu::RenderPipeline {
        if let Some((_, p)) = self.pipelines.iter().find(|(f, _)| *f == format) {
            return p.clone();
        }
        let p = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("app overlay"),
            layout: Some(&self.layout),
            vertex: wgpu::VertexState {
                module: &self.module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &self.module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        self.pipelines.push((format, p.clone()));
        p
    }

    /// Records a pass drawing `notice` in the top-left of `dst` (size
    /// `dst_size`), over what is there.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        notice: &Notice,
        dst: &wgpu::TextureView,
        dst_format: wgpu::TextureFormat,
        dst_size: (u32, u32),
    ) {
        if self.cached.as_ref().is_none_or(|(n, _, _)| n != notice) {
            let (w, h, px) = rasterize(notice);
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("app overlay"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                tex.as_image_copy(),
                &px,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 4),
                    rows_per_image: Some(h),
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
            let view = tex.create_view(&Default::default());
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("app overlay"),
                layout: &self.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            self.cached = Some((notice.clone(), bind, (w, h)));
        }
        let pipeline = self.pipeline(device, dst_format);
        let Some((_, bind, (w, h))) = &self.cached else {
            return;
        };
        let margin = 12u32;
        let (w, h) = (
            (*w).min(dst_size.0.saturating_sub(margin)),
            (*h).min(dst_size.1.saturating_sub(margin)),
        );
        if w == 0 || h == 0 {
            return;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("app overlay"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst,
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
        pass.set_viewport(margin as f32, margin as f32, w as f32, h as f32, 0.0, 1.0);
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(kind: NoticeKind, text: &str) -> Option<Notice> {
        Some(Notice {
            kind,
            text: text.into(),
        })
    }

    #[test]
    fn dismiss_hides_until_a_different_notice() {
        let b = Banner::new();
        assert!(b.visible().is_none());
        b.set(notice(NoticeKind::Info, "Building abc1234..."));
        let g = b.generation();
        b.dismiss();
        assert!(b.visible().is_none());
        assert!(b.generation() > g);
        // The same notice again stays hidden; a new one shows.
        b.set(notice(NoticeKind::Info, "Building abc1234..."));
        assert!(b.visible().is_none());
        b.set(notice(
            NoticeKind::Ready,
            "Update ready: 2 commits - press F5 to restart",
        ));
        assert_eq!(b.visible().unwrap().kind, NoticeKind::Ready);
    }

    #[test]
    fn restart_is_offered_only_by_a_ready_notice_even_when_hidden() {
        let b = Banner::new();
        b.set(notice(
            NoticeKind::Error,
            "Update failed to build: error[E0425]",
        ));
        assert!(!b.restart_offered());
        b.set(notice(NoticeKind::Ready, "Update ready"));
        b.dismiss();
        assert!(b.restart_offered());
        b.set(None);
        assert!(!b.restart_offered());
    }

    #[test]
    fn rasterize_draws_glyphs_on_a_plate() {
        let (w, h, px) = rasterize(&Notice {
            kind: NoticeKind::Ready,
            text: "Update ready \u{2014} F5\u{2026}".into(),
        });
        let chars = "Update ready - F5...".len() as u32;
        assert_eq!((w, h), (chars * 16 + 12, 28));
        assert_eq!(px.len(), (w * h * 4) as usize);
        let white = px.chunks(4).filter(|p| *p == [240, 240, 240, 255]).count();
        assert!(white > 100, "{white} glyph pixels");
        // Corners are the plate.
        assert_eq!(&px[0..4], &[16, 72, 32, 225]);
    }

    #[test]
    fn draws_over_the_frame_in_the_top_left() {
        let Ok(ctx) = gfx::GfxContext::headless() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let (w, h) = (640u32, 120u32);
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let tex = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        // The "game frame": solid blue.
        enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLUE),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let notice = Notice {
            kind: NoticeKind::Error,
            text: "Update failed".into(),
        };
        let mut overlay = OverlayRenderer::new(&ctx.device);
        overlay.draw(
            &ctx.device,
            &ctx.queue,
            &mut enc,
            &notice,
            &view,
            format,
            (w, h),
        );
        ctx.queue.submit([enc.finish()]);
        let px = gfx::readback::read_texture(&ctx, &tex, wgpu::TextureAspect::All, 4, (w, h));
        let at = |x: u32, y: u32| &px[((y * w + x) * 4) as usize..][..4];
        // Outside the plate the frame shows through untouched.
        assert_eq!(at(2, 2), &[0, 0, 255, 255]);
        assert_eq!(at(w - 2, h - 2), &[0, 0, 255, 255]);
        // Inside, the red plate blended over blue (alpha 225/255).
        let p = at(14, 14);
        assert!(p[0] > 70 && p[0] < 100 && p[2] > 20 && p[2] < 60, "{p:?}");
    }

    #[test]
    fn long_text_is_cut() {
        let s = ascii(&"x".repeat(500));
        assert_eq!(s.len(), MAX_CHARS);
        assert!(s.ends_with("..."));
    }
}
