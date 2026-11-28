//! Render targets and the windowing-agnostic presenter.
//!
//! The core never sees a window. A platform layer (winit on macOS today; a
//! UIKit/tvOS or Xbox shell later) creates a `wgpu::Surface` from whatever
//! window object it has, hands it to [`Presenter::new`] together with the
//! pixel size, and forwards resize events to [`Presenter::resize`]. Anything
//! that implements `raw-window-handle`'s `HasWindowHandle + HasDisplayHandle`
//! (via `wgpu::SurfaceTarget`) works.

use crate::context::{GfxContext, GfxError};

/// Depth/stencil format used for the backbuffer's depth attachment. The
/// Xbox's `D24S8` is the common case, so it is the default.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

/// Where a frame is drawn: views plus the formats pipelines must match.
pub struct RenderTarget<'a> {
    /// Colour attachment view.
    pub color: &'a wgpu::TextureView,
    /// Colour format.
    pub color_format: wgpu::TextureFormat,
    /// Depth/stencil attachment view.
    pub depth: Option<&'a wgpu::TextureView>,
    /// Depth/stencil format.
    pub depth_format: Option<wgpu::TextureFormat>,
    /// Size in pixels.
    pub size: (u32, u32),
    /// MSAA sample count of both attachments.
    pub samples: u32,
}

/// `usage` beyond `RENDER_ATTACHMENT`: an offscreen target's depth is
/// also `TEXTURE_BINDING`, so the native world's depth pyramid can read it
/// (docs/graphics/native-world.md, "Cull").
fn create_depth(device: &wgpu::Device, size: (u32, u32), usage: wgpu::TextureUsages) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("gfx depth"),
            size: wgpu::Extent3d { width: size.0.max(1), height: size.1.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | usage,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

/// An offscreen colour+depth target (tests, screenshots, render-to-texture).
pub struct OffscreenTarget {
    /// The colour texture (readable via `COPY_SRC`).
    pub texture: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    format: wgpu::TextureFormat,
    size: (u32, u32),
}

impl OffscreenTarget {
    /// Creates a target of `size` pixels in `format`.
    pub fn new(device: &wgpu::Device, size: (u32, u32), format: wgpu::TextureFormat) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gfx offscreen color"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let color_view = texture.create_view(&Default::default());
        Self { texture, color_view, depth_view: create_depth(device, size, wgpu::TextureUsages::TEXTURE_BINDING), format, size }
    }

    /// Borrows as a [`RenderTarget`].
    pub fn target(&self) -> RenderTarget<'_> {
        RenderTarget {
            color: &self.color_view,
            color_format: self.format,
            depth: Some(&self.depth_view),
            depth_format: Some(DEPTH_FORMAT),
            size: self.size,
            samples: 1,
        }
    }

    /// Copies the colour texture to the CPU as tightly packed rows (blocking).
    /// Only 4-byte-per-pixel formats are supported.
    pub fn read_pixels(&self, ctx: &GfxContext) -> Vec<u8> {
        crate::readback::read_texture(ctx, &self.texture, wgpu::TextureAspect::All, 4, self.size)
    }
}

/// Outcome of [`Presenter::acquire`].
pub enum Acquired {
    /// A frame to draw into.
    Frame(Frame),
    /// Nothing to draw this tick (occluded, minimised, or the surface was
    /// just reconfigured); try again next frame.
    Skip,
}

/// A frame being drawn. Get its [`RenderTarget`] from [`Presenter::target`],
/// record into it, then hand it back to [`Presenter::present`].
pub struct Frame(FrameOf);

enum FrameOf {
    Surface { surface_texture: wgpu::SurfaceTexture, color_view: wgpu::TextureView },
    /// A [`Presenter::sink`]'s frame: its offscreen target.
    Sink,
}

/// Where a [`Presenter::sink`] hands each finished frame: the colour view
/// and its size, on the presenter's device, after its work is submitted.
pub type Publish = Box<dyn FnMut(&GfxContext, &wgpu::TextureView, (u32, u32)) + Send>;

/// Presents to a window surface, or hands each frame to a sink that
/// composites it there (the app shell's frame channel).
pub struct Presenter(Out);

enum Out {
    Window { surface: wgpu::Surface<'static>, config: wgpu::SurfaceConfiguration, depth_view: wgpu::TextureView },
    Sink { target: OffscreenTarget, publish: Publish },
}

/// The format a sink's offscreen target draws in.
pub const SINK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl Presenter {
    /// Configures `surface` for `size` physical pixels.
    ///
    /// Picks an sRGB-agnostic (non-sRGB) format when available: the Xbox
    /// pipeline works in gamma-encoded space throughout, so an sRGB surface
    /// would double-encode.
    pub fn new(ctx: &GfxContext, surface: wgpu::Surface<'static>, size: (u32, u32)) -> Result<Self, GfxError> {
        let caps = surface.get_capabilities(&ctx.adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .ok_or_else(|| GfxError::Frame("surface reports no formats".into()))?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.0.max(1),
            height: size.1.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes.first().copied().unwrap_or(wgpu::CompositeAlphaMode::Auto),
            color_space: wgpu::SurfaceColorSpace::Auto,
            view_formats: vec![],
        };
        surface.configure(&ctx.device, &config);
        let depth_view = create_depth(&ctx.device, size, wgpu::TextureUsages::empty());
        Ok(Self(Out::Window { surface, config, depth_view }))
    }

    /// Draws each frame into an offscreen target of `size` pixels and hands
    /// it to `publish` at [`Presenter::present`].
    pub fn sink(ctx: &GfxContext, size: (u32, u32), publish: Publish) -> Self {
        let size = (size.0.max(1), size.1.max(1));
        Self(Out::Sink { target: OffscreenTarget::new(&ctx.device, size, SINK_FORMAT), publish })
    }

    /// The colour format (what pipelines must target).
    pub fn format(&self) -> wgpu::TextureFormat {
        match &self.0 {
            Out::Window { config, .. } => config.format,
            Out::Sink { .. } => SINK_FORMAT,
        }
    }

    /// Current size in pixels.
    pub fn size(&self) -> (u32, u32) {
        match &self.0 {
            Out::Window { config, .. } => (config.width, config.height),
            Out::Sink { target, .. } => target.size,
        }
    }

    /// Reconfigures for a new size. Zero sizes (minimised) are ignored.
    pub fn resize(&mut self, ctx: &GfxContext, size: (u32, u32)) {
        if size.0 == 0 || size.1 == 0 || size == self.size() {
            return;
        }
        match &mut self.0 {
            Out::Window { surface, config, depth_view } => {
                config.width = size.0;
                config.height = size.1;
                surface.configure(&ctx.device, config);
                *depth_view = create_depth(&ctx.device, size, wgpu::TextureUsages::empty());
            }
            Out::Sink { target, .. } => *target = OffscreenTarget::new(&ctx.device, size, SINK_FORMAT),
        }
    }

    /// Gets the next frame, reconfiguring the surface if it was lost.
    pub fn acquire(&mut self, ctx: &GfxContext) -> Result<Acquired, GfxError> {
        use wgpu::CurrentSurfaceTexture as C;
        let (surface, config) = match &self.0 {
            Out::Window { surface, config, .. } => (surface, config),
            Out::Sink { .. } => return Ok(Acquired::Frame(Frame(FrameOf::Sink))),
        };
        match surface.get_current_texture() {
            C::Success(t) | C::Suboptimal(t) => {
                let color_view = t.texture.create_view(&Default::default());
                Ok(Acquired::Frame(Frame(FrameOf::Surface { surface_texture: t, color_view })))
            }
            C::Outdated | C::Lost => {
                surface.configure(&ctx.device, config);
                Ok(Acquired::Skip)
            }
            C::Timeout | C::Occluded => Ok(Acquired::Skip),
            other => Err(GfxError::Frame(format!("{other:?}"))),
        }
    }

    /// The render target for an acquired frame.
    pub fn target<'a>(&'a self, frame: &'a Frame) -> RenderTarget<'a> {
        match (&self.0, &frame.0) {
            (Out::Window { config, depth_view, .. }, FrameOf::Surface { color_view, .. }) => RenderTarget {
                color: color_view,
                color_format: config.format,
                depth: Some(depth_view),
                depth_format: Some(DEPTH_FORMAT),
                size: (config.width, config.height),
                samples: 1,
            },
            (Out::Sink { target, .. }, FrameOf::Sink) => target.target(),
            _ => unreachable!("presenter: a frame from another presenter"),
        }
    }

    /// Presents a finished frame: the window's, or the sink's publish.
    pub fn present(&mut self, ctx: &GfxContext, frame: Frame) {
        match (&mut self.0, frame.0) {
            (Out::Window { .. }, FrameOf::Surface { surface_texture, .. }) => ctx.queue.present(surface_texture),
            (Out::Sink { target, publish }, FrameOf::Sink) => publish(ctx, &target.color_view, target.size),
            _ => unreachable!("presenter: a frame from another presenter"),
        }
    }
}
