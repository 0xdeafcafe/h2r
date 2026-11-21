//! Device acquisition. Windowing-agnostic: the only thing a platform layer
//! has to supply is a `wgpu::Surface` (or nothing, for headless use).

use crate::translate::Capabilities;
pub use mem_watch::gpu::DeviceRegistration;

/// Errors from bringing up the GPU.
#[derive(Debug, thiserror::Error)]
pub enum GfxError {
    /// No adapter satisfied the request.
    #[error("no suitable GPU adapter: {0}")]
    Adapter(#[from] wgpu::RequestAdapterError),
    /// The adapter refused to create a device.
    #[error("device request failed: {0}")]
    Device(#[from] wgpu::RequestDeviceError),
    /// Surface creation failed.
    #[error("surface creation failed: {0}")]
    Surface(#[from] wgpu::CreateSurfaceError),
    /// Acquiring the next frame failed unrecoverably.
    #[error("surface error: {0}")]
    Frame(String),
}

/// Owns the wgpu instance, adapter, device and queue. Clones share the device.
#[derive(Clone)]
pub struct GfxContext {
    /// The instance (kept alive for the surface's lifetime).
    pub instance: wgpu::Instance,
    /// The chosen adapter.
    pub adapter: wgpu::Adapter,
    /// The logical device.
    pub device: wgpu::Device,
    /// The submission queue.
    pub queue: wgpu::Queue,
    /// Optional features that were enabled.
    pub caps: Capabilities,
}

impl GfxContext {
    /// Keep this device visible to the memory census while its owner is active.
    pub fn watch_resources(&self) -> DeviceRegistration {
        DeviceRegistration::new(&self.instance, &self.adapter, &self.device)
    }

    /// Creates a context, preferring an adapter that can present to
    /// `compatible_surface`.
    pub async fn new(instance: wgpu::Instance, compatible_surface: Option<&wgpu::Surface<'_>>) -> Result<Self, GfxError> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface,
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await?;
        let features = adapter.features() & Capabilities::wanted_features();
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("gfx device"),
                required_features: features,
                ..Default::default()
            })
            .await?;
        Ok(Self { instance, adapter, device, queue, caps: Capabilities::from_features(features) })
    }

    /// Headless context (no surface), blocking.
    pub fn headless() -> Result<Self, GfxError> {
        pollster::block_on(Self::new(instance(), None))
    }
}

/// Which kind of host a wgpu instance is made for: the one place the
/// backends differ between them (docs/runtime/switches.md, `WGPU_BACKEND`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuHost {
    /// macOS, and the tvOS and iOS apps: every backend wgpu was built with,
    /// which is Metal.
    Apple,
    /// A Windows desktop window: every backend.
    Windows,
    /// The UWP app (Xbox Dev Mode, docs/runtime/windows.md): DX12 alone, and
    /// no `WGPU_BACKEND`. wgpu's GL backend imports `opengl32.dll`, which an
    /// Xbox app container lacks: it is delay-loaded (platform/xbox/build.sh)
    /// and must never be called.
    WindowsApp,
    /// Linux and every other Unix: Vulkan alone, so adapter selection cannot
    /// land on GLES, which cannot present on Wayland without a display
    /// handle.
    Unix,
}

impl GpuHost {
    /// The desktop host this binary is built for: Apple targets and Windows
    /// are named, every other target is Unix. An embedded host names its
    /// own (`app::embedded::host_surface::GPU`).
    pub const DESKTOP: Self = if cfg!(target_vendor = "apple") {
        Self::Apple
    } else if cfg!(windows) {
        Self::Windows
    } else {
        Self::Unix
    };

    /// The backends used when `WGPU_BACKEND` is unset or not read.
    pub fn backends(self) -> wgpu::Backends {
        match self {
            Self::Unix => wgpu::Backends::VULKAN,
            Self::WindowsApp => wgpu::Backends::DX12,
            Self::Apple | Self::Windows => wgpu::Backends::all(),
        }
    }

    /// Whether `WGPU_BACKEND` may replace [`backends`](Self::backends).
    pub fn reads_env(self) -> bool {
        !matches!(self, Self::WindowsApp)
    }
}

/// The wgpu instance for `host`: `WGPU_BACKEND` when set and the host reads
/// it, else [`GpuHost::backends`].
pub fn instance_for(host: GpuHost) -> wgpu::Instance {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    let from_env = if host.reads_env() { wgpu::Backends::from_env() } else { None };
    desc.backends = from_env.unwrap_or_else(|| host.backends());
    if host == GpuHost::WindowsApp {
        // The swap chain kind and whether acquire waits on DXGI's frame
        // latency object, once (the Xbox's frame pacing, docs/runtime/xbox.md).
        eprintln!(
            "app: dx12 presentation {:?}, frame latency waitable object {:?}",
            desc.backend_options.dx12.presentation_system, desc.backend_options.dx12.latency_waitable_object
        );
    }
    wgpu::Instance::new(desc)
}

/// The wgpu instance every desktop window, headless context, tool and GPU
/// test starts from: [`instance_for`] the [`GpuHost::DESKTOP`] host.
pub fn instance() -> wgpu::Instance {
    instance_for(GpuHost::DESKTOP)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_host_has_its_backends() {
        let table = [
            (GpuHost::Apple, wgpu::Backends::all(), true),
            (GpuHost::Windows, wgpu::Backends::all(), true),
            // No GL on the Xbox: opengl32.dll is not in the app container.
            (GpuHost::WindowsApp, wgpu::Backends::DX12, false),
            (GpuHost::Unix, wgpu::Backends::VULKAN, true),
        ];
        for (host, backends, reads_env) in table {
            assert_eq!(host.backends(), backends, "{host:?}");
            assert_eq!(host.reads_env(), reads_env, "{host:?}");
        }
    }

    #[test]
    fn the_desktop_host() {
        let want = if cfg!(target_vendor = "apple") {
            GpuHost::Apple
        } else if cfg!(windows) {
            GpuHost::Windows
        } else {
            GpuHost::Unix
        };
        assert_eq!(GpuHost::DESKTOP, want);
    }
}
