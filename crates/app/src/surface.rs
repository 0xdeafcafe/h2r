//! The swapchain the shell presents into: a winit window's (macOS) or a
//! `CAMetalLayer` a native host hands over (tvOS, [`crate::embedded`]).

use gfx::GfxContext;

/// The window's swapchain. gfx's `Presenter` always uses `AutoVsync` and
/// allocates a depth buffer; the shell needs neither restriction.
pub(crate) struct WindowSurface {
    pub(crate) surface: wgpu::Surface<'static>,
    pub(crate) config: wgpu::SurfaceConfiguration,
    /// The surface needs `configure` before the next acquire: the window
    /// was resized, or the last configure failed (wgpu could not wait for
    /// the GPU to go idle, which happens mid-resize under a heavy frame).
    stale: bool,
}

impl WindowSurface {
    pub(crate) fn new(
        ctx: &GfxContext,
        surface: wgpu::Surface<'static>,
        size: (u32, u32),
        vsync: bool,
    ) -> anyhow::Result<Self> {
        let caps = surface.get_capabilities(&ctx.adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .ok_or_else(|| anyhow::anyhow!("window surface reports no formats"))?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.0.max(1),
            height: size.1.max(1),
            present_mode: if vsync {
                wgpu::PresentMode::AutoVsync
            } else {
                wgpu::PresentMode::AutoNoVsync
            },
            desired_maximum_frame_latency: 2,
            alpha_mode: caps
                .alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto),
            color_space: wgpu::SurfaceColorSpace::Auto,
            view_formats: vec![],
        };
        surface.configure(&ctx.device, &config);
        Ok(Self {
            surface,
            config,
            stale: false,
        })
    }

    pub(crate) fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub(crate) fn resize(&mut self, size: (u32, u32)) {
        if size.0 == 0 || size.1 == 0 || size == self.size() {
            return;
        }
        // Only recorded: reconfiguring inside the window's resize callback
        // waits for the GPU to go idle there, and a failure in that
        // callback cannot unwind (it aborted the process). The next acquire
        // configures, inside an error scope.
        self.config.width = size.0;
        self.config.height = size.1;
        self.stale = true;
    }

    /// Configures the surface, catching a validation error (a GPU that did
    /// not go idle in time) instead of panicking; `stale` stays set so the
    /// next acquire tries again.
    pub(crate) fn configure(&mut self, ctx: &GfxContext) -> bool {
        let scope = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
        self.surface.configure(&ctx.device, &self.config);
        match pollster::block_on(scope.pop()) {
            None => {
                self.stale = false;
                true
            }
            Some(e) => {
                eprintln!("app: surface configure failed, retrying next frame: {e}");
                self.stale = true;
                false
            }
        }
    }

    /// Waits for the display's vblank (`true`) or not, from the next frame.
    #[cfg_attr(target_os = "tvos", allow(dead_code))]
    pub(crate) fn set_vsync(&mut self, vsync: bool) {
        self.config.present_mode = if vsync {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        // Deferred like a resize: the next acquire configures inside an
        // error scope, so a GPU that is slow to go idle cannot abort us.
        self.stale = true;
    }

    pub(crate) fn acquire(&mut self, ctx: &GfxContext) -> Option<wgpu::SurfaceTexture> {
        use wgpu::CurrentSurfaceTexture as C;
        if self.stale && !self.configure(ctx) {
            return None;
        }
        match self.surface.get_current_texture() {
            C::Success(t) | C::Suboptimal(t) => Some(t),
            C::Outdated | C::Lost => {
                self.configure(ctx);
                None
            }
            C::Timeout | C::Occluded => None,
            other => {
                eprintln!("app: surface: {other:?}");
                None
            }
        }
    }
}
