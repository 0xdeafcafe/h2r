//! The swapchain the shell presents into: a winit window's (desktop) or a
//! `CAMetalLayer` a native host hands over (tvOS, [`crate::embedded`]).

use gfx::GfxContext;

use crate::frame::{MAX_FRAME_DIMENSION, ShownFrame};
use crate::scale::{NativeSize, Viewport, limit_drawable};
use crate::AppConfig;

/// What a surface presents into, as far as choosing its alpha mode goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SurfaceKind {
    /// A desktop window (a winit window's HWND, NSView or Wayland surface)
    /// or a `CAMetalLayer` a native host hands over: `Opaque` when listed.
    Window,
    /// A DXGI composition swap chain on a XAML `SwapChainPanel` (the UWP
    /// app, [`crate::embedded`]): the first mode listed, which is `Auto`,
    /// so the panel's own composition decides.
    Composition,
}

/// The window's swapchain. gfx's `Presenter` always uses `AutoVsync` and
/// allocates a depth buffer; the shell needs neither restriction.
pub(crate) struct WindowSurface {
    pub(crate) surface: wgpu::Surface<'static>,
    pub(crate) native_size: NativeSize,
    dimension_limit: u32,
    pub(crate) config: wgpu::SurfaceConfiguration,
    /// The surface needs `configure` before the next acquire: the window
    /// was resized, or the last configure failed (wgpu could not wait for
    /// the GPU to go idle, which happens mid-resize under a heavy frame).
    stale: bool,
    last_configure_error: Option<String>,
}

impl WindowSurface {
    pub(crate) fn new(
        ctx: &GfxContext,
        surface: wgpu::Surface<'static>,
        size: (u32, u32),
        app: &AppConfig,
        kind: SurfaceKind,
    ) -> anyhow::Result<Self> {
        let caps = surface.get_capabilities(&ctx.adapter);
        let format = surface_format(&caps.formats)
            .ok_or_else(|| anyhow::anyhow!("window surface reports no formats"))?;
        let dimension_limit = ctx.device.limits().max_texture_dimension_2d.min(MAX_FRAME_DIMENSION);
        let size = limit_drawable(size, dimension_limit);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.0.clamp(1, dimension_limit),
            height: size.1.clamp(1, dimension_limit),
            present_mode: if app.fps.vsync(app.vsync) {
                wgpu::PresentMode::AutoVsync
            } else {
                wgpu::PresentMode::AutoNoVsync
            },
            desired_maximum_frame_latency: 2,
            alpha_mode: alpha_mode(&caps.alpha_modes, kind),
            color_space: wgpu::SurfaceColorSpace::Auto,
            view_formats: vec![],
        };
        surface.configure(&ctx.device, &config);
        Ok(Self {
            surface,
            native_size: NativeSize::new(app.aspect, app.scaling, size)
                .with_native_fit(app.native_fit.clone()),
            dimension_limit,
            config,
            stale: false,
            last_configure_error: None,
        })
    }

    pub(crate) fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub(crate) fn viewport(&self, frame: Option<&ShownFrame>) -> Viewport {
        frame.map_or_else(Viewport::default, |frame| {
            self.native_size.viewport(frame.size, self.size(), frame.loading)
        })
    }

    pub(crate) fn resize(&mut self, size: (u32, u32)) {
        if size.0 == 0 || size.1 == 0 {
            return;
        }
        let size = limit_drawable(size, self.dimension_limit);
        self.native_size.update(size);
        if size == self.size() {
            return;
        }
        // Acquire configures inside an error scope, on this redraw for a
        // desktop resize and on the next display-link tick for embedded.
        self.config.width = size.0;
        self.config.height = size.1;
        self.stale = true;
    }

    /// Configures the surface, catching a validation error (a GPU that did
    /// not go idle in time) instead of panicking; `stale` stays set so the
    /// next acquire tries again.
    fn configure(&mut self, ctx: &GfxContext) -> bool {
        let scope = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
        self.surface.configure(&ctx.device, &self.config);
        match pollster::block_on(scope.pop()) {
            None => {
                self.last_configure_error = None;
                self.stale = false;
                true
            }
            Some(e) => {
                let message = e.to_string();
                if self.last_configure_error.as_ref() != Some(&message) {
                    eprintln!("app: surface configure failed, retrying on redraw: {message}");
                    self.last_configure_error = Some(message);
                }
                self.stale = true;
                false
            }
        }
    }

    #[cfg(not(any(target_os = "tvos", target_os = "ios")))]
    pub(crate) fn needs_configure(&self) -> bool {
        self.stale
    }

    /// Waits for the display's vblank (`true`) or not, from the next frame.
    #[cfg_attr(any(target_os = "tvos", target_os = "ios"), allow(dead_code))]
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
                self.stale = true;
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

/// Vulkan drivers and compositors may list 10-bit or float formats first;
/// the compositor is written for 8-bit unorm output.
fn surface_format(formats: &[wgpu::TextureFormat]) -> Option<wgpu::TextureFormat> {
    [wgpu::TextureFormat::Bgra8Unorm, wgpu::TextureFormat::Rgba8Unorm]
        .into_iter()
        .find(|f| formats.contains(f))
        .or_else(|| formats.iter().copied().find(|f| !f.is_srgb()))
        .or_else(|| formats.first().copied())
}

/// `Opaque` for a window when it is listed: the frame is never meant to
/// show what is behind it. A composition swap chain keeps the first mode.
fn alpha_mode(modes: &[wgpu::CompositeAlphaMode], kind: SurfaceKind) -> wgpu::CompositeAlphaMode {
    let first = modes.first().copied().unwrap_or(wgpu::CompositeAlphaMode::Auto);
    match kind {
        SurfaceKind::Window if modes.contains(&wgpu::CompositeAlphaMode::Opaque) => {
            wgpu::CompositeAlphaMode::Opaque
        }
        SurfaceKind::Window | SurfaceKind::Composition => first,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::CompositeAlphaMode as A;
    use wgpu::TextureFormat as F;

    // The lists wgpu-hal 30 reports, in its order.
    /// Metal (`metal/adapter.rs`), with the 10-bit format a recent GPU adds.
    const METAL_FORMATS: &[F] = &[F::Bgra8Unorm, F::Bgra8UnormSrgb, F::Rgba16Float, F::Rgb10a2Unorm];
    const METAL_ALPHA: &[A] = &[A::Opaque, A::PostMultiplied];
    /// DX12 (`dx12/adapter.rs`): the same formats for every target.
    const DX12_FORMATS: &[F] =
        &[F::Bgra8UnormSrgb, F::Bgra8Unorm, F::Rgba8UnormSrgb, F::Rgba8Unorm, F::Rgb10a2Unorm, F::Rgba16Float];
    /// DX12 on an HWND.
    const DX12_HWND_ALPHA: &[A] = &[A::Opaque];
    /// DX12 on a visual or a `SwapChainPanel`.
    const DX12_COMPOSITION_ALPHA: &[A] = &[A::Auto, A::Inherit, A::Opaque, A::PostMultiplied, A::PreMultiplied];
    /// Vulkan: whatever the driver lists, in its order. An HDR-capable
    /// Wayland compositor lists 10-bit and float first.
    const VULKAN_10_BIT_FIRST: &[F] = &[F::Rgb10a2Unorm, F::Rgba16Float, F::Bgra8UnormSrgb, F::Bgra8Unorm];
    /// Vulkan's alpha modes in `map_vk_composite_alpha`'s order: a Wayland
    /// compositor with and without `OPAQUE`.
    const VULKAN_ALPHA: &[A] = &[A::Opaque, A::PreMultiplied];
    const VULKAN_ALPHA_NO_OPAQUE: &[A] = &[A::PreMultiplied, A::Inherit];

    #[test]
    fn the_surface_format_is_8_bit_unorm_when_listed() {
        let table: &[(&str, &[F], Option<F>)] = &[
            ("metal", METAL_FORMATS, Some(F::Bgra8Unorm)),
            ("dx12", DX12_FORMATS, Some(F::Bgra8Unorm)),
            ("vulkan 10-bit first", VULKAN_10_BIT_FIRST, Some(F::Bgra8Unorm)),
            ("rgba only", &[F::Rgba8UnormSrgb, F::Rgba8Unorm], Some(F::Rgba8Unorm)),
            ("no 8-bit unorm", &[F::Bgra8UnormSrgb, F::Rgb10a2Unorm], Some(F::Rgb10a2Unorm)),
            ("srgb only", &[F::Bgra8UnormSrgb], Some(F::Bgra8UnormSrgb)),
            ("none", &[], None),
        ];
        for (name, formats, want) in table {
            assert_eq!(surface_format(formats), *want, "{name}");
        }
    }

    #[test]
    fn the_alpha_mode_is_opaque_except_on_a_composition_swap_chain() {
        use SurfaceKind::{Composition, Window};
        let table: &[(&str, &[A], SurfaceKind, A)] = &[
            ("metal", METAL_ALPHA, Window, A::Opaque),
            ("dx12 hwnd", DX12_HWND_ALPHA, Window, A::Opaque),
            // The UWP SwapChainPanel keeps the first mode, as it always has.
            ("dx12 swapchainpanel", DX12_COMPOSITION_ALPHA, Composition, A::Auto),
            ("vulkan", VULKAN_ALPHA, Window, A::Opaque),
            ("vulkan without opaque", VULKAN_ALPHA_NO_OPAQUE, Window, A::PreMultiplied),
            ("composition without auto", &[A::Opaque, A::PreMultiplied], Composition, A::Opaque),
            ("none listed", &[], Window, A::Auto),
            ("none listed, composition", &[], Composition, A::Auto),
        ];
        for (name, modes, kind, want) in table {
            assert_eq!(alpha_mode(modes, *kind), *want, "{name}");
        }
    }
}
