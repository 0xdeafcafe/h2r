//! Text plates drawn over the game frame: the dismissable notice and the
//! paired editor's status.
//!
//! [`Banner`] is a shared handle: any thread sets the notice, the window
//! draws it. Text is rasterised on the CPU with the public-domain `font8x8`
//! glyphs (ASCII; a few typographic characters are mapped to ASCII), scaled
//! 2x, on a translucent plate, and drawn in the top-left corner of the
//! window with alpha blending. A notice's text may hold several lines
//! (`\n`). F6 hides the current notice (a new notice shows again); F5 asks
//! for a restart while the notice offers one.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

pub mod marks;

/// What a notice is about; picks its colour and whether F5 restarts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    /// Work in progress ("Building ...").
    Info,
    /// An update is ready: F5 restarts into it.
    Ready,
    /// Something failed; the game keeps running as it is.
    Error,
    /// The game itself stopped (the guest thread ended): what happened,
    /// and F5 starts it again (docs/runtime/app.md "When the game stops").
    Crashed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub kind: NoticeKind,
    pub text: String,
}

/// One or two lines of paired editor status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorPlate {
    pub title: &'static str,
    pub text: Option<String>,
    pub tone: NoticeKind,
}

impl EditorPlate {
    pub fn lines(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.title).chain(self.text.as_deref().filter(|text| !text.is_empty()))
    }
}

/// The window reads the plate and its wall-clock deadline, and consumes focus
/// requests. The source wakes the event loop when either changes.
pub trait EditorPlateSource: std::fmt::Debug + Send + Sync {
    fn current(&self, now: Instant) -> Option<(EditorPlate, Instant)>;
    fn set_waker(&self, wake: Arc<dyn Fn() + Send + Sync>);
    fn take_focus_request(&self) -> bool;
    fn set_mark_frame_status(&self, status: marks::MarkFrameStatus);
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
            .is_some_and(|n| matches!(n.kind, NoticeKind::Ready | NoticeKind::Crashed))
    }

    pub fn generation(&self) -> u64 {
        self.lock().generation
    }
}

/// Lines of a crash message the stopped notice shows; the terminal has
/// the rest.
const STOPPED_LINES: usize = 6;
/// Width the stopped notice wraps its message to.
const STOPPED_WRAP: usize = 88;

/// The notice the window shows when the game itself stops while the window
/// stays open: what happened, and that F5 starts it again. `None` for an
/// exit that isn't the game stopping on its own.
pub fn stopped_notice(exit: &crate::GuestExit) -> Option<Notice> {
    use crate::GuestExit;
    let (head, message) = match exit {
        GuestExit::Returned => ("The game stopped.", None),
        GuestExit::Failed(m) => ("The game crashed.", Some(m.as_str())),
        GuestExit::Panicked(m) => ("The game crashed (host panic).", Some(m.as_str())),
        GuestExit::StillRunning | GuestExit::NotStarted => return None,
    };
    let mut lines = vec![head.to_owned()];
    if let Some(m) = message {
        let wrapped = wrap(m, STOPPED_WRAP);
        let more = wrapped.len() > STOPPED_LINES;
        lines.extend(wrapped.into_iter().take(STOPPED_LINES));
        if more {
            lines.push("... (the rest is in the terminal)".into());
        }
    }
    lines.push(String::new());
    lines.push("Press F5 to start the game again, or close the window to quit.".into());
    Some(Notice {
        kind: NoticeKind::Crashed,
        text: lines.join("\n"),
    })
}

/// `text` broken into lines of at most `width` characters, at spaces
/// where it can be; an error chain's `: ` joins break there too.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.lines().map(str::trim_end).filter(|l| !l.is_empty()) {
        let mut line = String::new();
        for word in para.split(' ') {
            let mut word = word.to_owned();
            while word.chars().count() > width {
                let head: String = word.chars().take(width).collect();
                word = word.chars().skip(width).collect();
                if !line.is_empty() {
                    out.push(std::mem::take(&mut line));
                }
                out.push(head);
            }
            let need = line.chars().count() + usize::from(!line.is_empty()) + word.chars().count();
            if need > width && !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&word);
        }
        if !line.is_empty() {
            out.push(line);
        }
    }
    out
}

/// Glyph scale and padding, in window pixels.
const SCALE: u32 = 2;
const PAD: u32 = 6;
/// Longest line drawn; the rest is cut with "...".
const MAX_CHARS: usize = 96;
const LINE_HEIGHT: u32 = 8 * SCALE;
const LINE_GAP: u32 = 4;
const MARGIN: u32 = 12;

/// Keep the middle dot and map common punctuation to the available ASCII glyphs.
fn overlay_text(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '·' => out.push(c),
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
    rasterize_lines(notice.kind, notice.text.lines())
}

fn rasterize_lines<'a>(
    tone: NoticeKind,
    lines: impl Iterator<Item = &'a str>,
) -> (u32, u32, Vec<u8>) {
    use font8x8::UnicodeFonts;

    let lines: Vec<_> = lines.map(overlay_text).collect();
    let cols = lines
        .iter()
        .map(|text| text.chars().count())
        .max()
        .unwrap_or(0);
    let rows = lines.len().max(1) as u32;
    let w = cols.max(1) as u32 * 8 * SCALE + 2 * PAD;
    let h = rows * LINE_HEIGHT + (rows - 1) * LINE_GAP + 2 * PAD;
    let plate: [u8; 4] = match tone {
        NoticeKind::Info => [24, 32, 48, 210],
        NoticeKind::Ready => [16, 72, 32, 225],
        NoticeKind::Error | NoticeKind::Crashed => [96, 20, 20, 225],
    };
    let mut px: Vec<u8> = plate
        .iter()
        .copied()
        .cycle()
        .take((w * h * 4) as usize)
        .collect();
    for (line, text) in lines.iter().enumerate() {
        let top = PAD + line as u32 * (LINE_HEIGHT + LINE_GAP);
        for (i, c) in text.chars().enumerate() {
            let Some(glyph) = font8x8::BASIC_FONTS
                .get(c)
                .or_else(|| font8x8::LATIN_FONTS.get(c))
            else {
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
                            let y = top + row as u32 * SCALE + dy;
                            let o = ((y * w + x) * 4) as usize;
                            px[o..o + 4].copy_from_slice(&[240, 240, 240, 255]);
                        }
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

/// A straight-alpha RGBA image drawn 1:1 into a rectangle of an already
/// composited target, alpha blended: one quad, one draw. Shared by the
/// banner and the watermark.
pub(crate) struct Sprite {
    module: wgpu::ShaderModule,
    bgl: wgpu::BindGroupLayout,
    layout: wgpu::PipelineLayout,
    sampler: wgpu::Sampler,
    pipelines: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    /// The uploaded image: texture, bind group and size.
    image: Option<(wgpu::Texture, wgpu::BindGroup, (u32, u32))>,
}

impl Sprite {
    pub(crate) fn new(device: &wgpu::Device, label: &'static str) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Wgsl(OVERLAY_WGSL.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(label),
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
            label: Some(label),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        // Nearest (the default): the image is drawn at its own size.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some(label),
            ..Default::default()
        });
        Self {
            module,
            bgl,
            layout,
            sampler,
            pipelines: Vec::new(),
            image: None,
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

    /// The uploaded image's size, if any.
    pub(crate) fn size(&self) -> Option<(u32, u32)> {
        self.image.as_ref().map(|(_, _, s)| *s)
    }

    /// Replaces the image with `px` (`w` x `h` straight-alpha RGBA). The
    /// texture is kept while the size stays the same.
    pub(crate) fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        (w, h): (u32, u32),
        px: &[u8],
    ) {
        if w == 0 || h == 0 {
            self.image = None;
            return;
        }
        let extent = wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        };
        if self.size() != Some((w, h)) {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("app overlay"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
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
            self.image = Some((tex, bind, (w, h)));
        }
        let (tex, _, _) = self.image.as_ref().expect("just set");
        queue.write_texture(
            tex.as_image_copy(),
            px,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            extent,
        );
    }

    /// Records a pass drawing the image with its top-left at `at`, within
    /// `dst_size`, over what is there.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        dst: &wgpu::TextureView,
        dst_format: wgpu::TextureFormat,
        dst_size: (u32, u32),
        at: (u32, u32),
    ) {
        let Some((width, height)) = self.size() else { return };
        // Preserve the existing unscaled draw, clamped to the target.
        if at.0 >= dst_size.0 || at.1 >= dst_size.1 {
            return;
        }
        self.draw_in(device, encoder, dst, dst_format, crate::scale::Viewport {
            x: at.0,
            y: at.1,
            width: width.min(dst_size.0 - at.0),
            height: height.min(dst_size.1 - at.1),
        });
    }

    /// Draw into an already fitted viewport.
    pub(crate) fn draw_in(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        dst: &wgpu::TextureView,
        dst_format: wgpu::TextureFormat,
        rect: crate::scale::Viewport,
    ) {
        if rect.is_empty() {
            return;
        }
        let pipeline = self.pipeline(device, dst_format);
        let Some((_, bind, _)) = &self.image else { return };
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
        pass.set_viewport(
            rect.x as f32,
            rect.y as f32,
            rect.width as f32,
            rect.height as f32,
            0.0,
            1.0,
        );
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Draws the banner's notice over an already composited target.
pub struct OverlayRenderer {
    sprite: Sprite,
    /// The last notice rasterised.
    cached: Option<Notice>,
}

impl OverlayRenderer {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            sprite: Sprite::new(device, "app overlay"),
            cached: None,
        }
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
        if self.cached.as_ref() != Some(notice) {
            let (w, h, px) = rasterize(notice);
            self.sprite.upload(device, queue, (w, h), &px);
            self.cached = Some(notice.clone());
        }
        self.sprite
            .draw(device, encoder, dst, dst_format, dst_size, (MARGIN, MARGIN));
    }
}

#[cfg(not(any(target_os = "tvos", target_os = "ios")))]
pub(crate) struct EditorPlateRenderer {
    sprite: Sprite,
    cached: Option<EditorPlate>,
}

#[cfg(not(any(target_os = "tvos", target_os = "ios")))]
impl EditorPlateRenderer {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        Self {
            sprite: Sprite::new(device, "editor plate"),
            cached: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        plate: &EditorPlate,
        banner_visible: bool,
        dst: &wgpu::TextureView,
        dst_format: wgpu::TextureFormat,
        dst_size: (u32, u32),
    ) {
        if self.cached.as_ref() != Some(plate) {
            let (w, h, px) = rasterize_lines(plate.tone, plate.lines());
            self.sprite.upload(device, queue, (w, h), &px);
            self.cached = Some(plate.clone());
        }
        let Some((width, _)) = self.sprite.size() else {
            return;
        };
        let x = dst_size.0.saturating_sub(width + MARGIN).max(MARGIN);
        // Reserve the banner's row when it is visible, even in narrow windows.
        let y = if banner_visible {
            2 * MARGIN + LINE_HEIGHT + 2 * PAD
        } else {
            MARGIN
        };
        self.sprite
            .draw(device, encoder, dst, dst_format, dst_size, (x, y));
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
    fn editor_plate_rasterizes_both_lines() {
        let plate = EditorPlate {
            title: "Editor · viewing",
            text: Some("Aim select".into()),
            tone: NoticeKind::Info,
        };
        let (w, h, px) = rasterize_lines(plate.tone, plate.lines());
        assert_eq!(h, 2 * LINE_HEIGHT + LINE_GAP + 2 * PAD);
        let ink = |x, y| {
            let i = ((y * w + x) * 4) as usize;
            px[i..i + 4] == [240, 240, 240, 255]
        };
        let dot_x = PAD + 7 * 8 * SCALE;
        assert!(
            (PAD..PAD + LINE_HEIGHT).any(|y| (dot_x..dot_x + 8 * SCALE).any(|x| ink(x, y)))
        );
        let second_y = PAD + LINE_HEIGHT + LINE_GAP;
        assert!((second_y..second_y + LINE_HEIGHT).any(|y| (PAD..w - PAD).any(|x| ink(x, y))));
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
        let s = overlay_text(&"x".repeat(500));
        assert_eq!(s.len(), MAX_CHARS);
        assert!(s.ends_with("..."));
    }

    #[test]
    fn a_crash_says_what_happened_and_offers_f5() {
        use crate::GuestExit;
        let long = format!("guest fault at 0x001c83d8: {}", "read of unmapped memory ".repeat(40));
        let n = stopped_notice(&GuestExit::Failed(long)).unwrap();
        assert_eq!(n.kind, NoticeKind::Crashed);
        let lines: Vec<_> = n.text.lines().collect();
        assert_eq!(lines[0], "The game crashed.");
        assert!(lines[1].starts_with("guest fault at 0x001c83d8:"));
        assert!(lines.iter().all(|l| l.chars().count() <= MAX_CHARS), "{lines:?}");
        assert!(lines.contains(&"... (the rest is in the terminal)"));
        assert!(lines.last().unwrap().starts_with("Press F5"));
        assert!(lines.len() <= STOPPED_LINES + 4);

        let b = Banner::new();
        b.set(Some(n));
        assert!(b.restart_offered());
        let (_, h, _) = rasterize(&b.visible().unwrap());
        assert!(h > 4 * LINE_HEIGHT, "draws every line");

        let quit = stopped_notice(&GuestExit::Returned).unwrap();
        assert!(quit.text.starts_with("The game stopped."));
        assert!(stopped_notice(&GuestExit::StillRunning).is_none());
        assert!(stopped_notice(&GuestExit::NotStarted).is_none());
    }

    #[test]
    fn wrapping_breaks_at_spaces_and_splits_long_words() {
        assert_eq!(wrap("aa bb cc", 5), ["aa bb", "cc"]);
        assert_eq!(wrap("abcdefgh", 3), ["abc", "def", "gh"]);
        assert_eq!(wrap("one\n\ntwo", 10), ["one", "two"]);
    }
}
