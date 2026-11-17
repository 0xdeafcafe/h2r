//! The shell for a native host that owns the window and the run loop
//! (the tvOS and iOS apps, platform/tvos and platform/ios; docs/runtime/tvos.md,
//! docs/runtime/ios.md; later the UWP app for Xbox, docs/runtime/windows.md), in place
//! of winit's.
//!
//! The host hands over its surface ([`host_surface`]) and calls [`Embedded::redraw`] from
//! its display link on the main thread; everything else is the windowed
//! shell's: the same [`GuestHost`] on a guest thread, the same frame
//! handoff, compositor and pacing. Input is GameController: pads, the
//! Apple TV remote and iOS's on-screen controls, behind the shell's usual
//! player-one pad (no key or mouse events arrive; the pad tracks the
//! device last used, for the button icons). Text typed into the game's
//! virtual keyboard comes from the host's system keyboard through
//! [`Embedded::keyboard`] (docs/input/host-keyboard.md).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gfx::GfxContext;

use crate::audio_out::{AudioOut, FALLBACK_RATE};
use crate::frame::{self, Compositor, FrameSource};
use crate::lifecycle::Lifecycle;
use crate::pacing::{PaceStats, Pacer};
use crate::scale;
use crate::surface::WindowSurface;
use crate::worker::{GuestFn, GuestWorker};
use crate::{AppConfig, ExitReport, GuestExit, GuestHost};

/// What a native host presents into, per platform: the one place
/// [`Embedded::start`] differs between them.
pub mod host_surface {
    /// The surface the host passes as `layer`.
    #[cfg(target_vendor = "apple")]
    pub const NAME: &str = "CAMetalLayer";
    /// The surface the host passes as `layer`: a XAML `SwapChainPanel`
    /// (its `IInspectable*`), which wgpu's DX12 backend binds a swap chain
    /// to through `ISwapChainPanelNative`. The UWP host (Xbox Dev Mode,
    /// docs/runtime/windows.md) owns it; wgpu 30 has no `CoreWindow` target.
    #[cfg(windows)]
    pub const NAME: &str = "SwapChainPanel";

    /// The backends the shell's instance may use: Metal on Apple
    /// platforms; on Windows, DX12 only. wgpu's GL backend imports
    /// `opengl32.dll`, which an Xbox app container lacks: it is
    /// delay-loaded (platform/xbox/build.sh) and must never be called.
    pub fn backends() -> wgpu::Backends {
        if cfg!(windows) { wgpu::Backends::DX12 } else { wgpu::Backends::all() }
    }

    /// The instance a host's surface is made on, with [`backends`].
    pub fn instance() -> wgpu::Instance {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = backends();
        // The swap chain kind and whether acquire waits on DXGI's frame
        // latency object, once (the Xbox's frame pacing, docs/runtime/xbox.md).
        #[cfg(windows)]
        eprintln!(
            "app: dx12 presentation {:?}, frame latency waitable object {:?}",
            desc.backend_options.dx12.presentation_system,
            desc.backend_options.dx12.latency_waitable_object
        );
        wgpu::Instance::new(desc)
    }

    /// The wgpu target for the host's `layer`.
    #[cfg(target_vendor = "apple")]
    pub fn target(layer: *mut std::ffi::c_void) -> wgpu::SurfaceTargetUnsafe {
        wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(layer)
    }
    /// The wgpu target for the host's `layer`.
    #[cfg(windows)]
    pub fn target(layer: *mut std::ffi::c_void) -> wgpu::SurfaceTargetUnsafe {
        wgpu::SurfaceTargetUnsafe::SwapChainPanel(layer)
    }
}

/// A running guest presenting into a host's layer. Main thread only.
pub struct Embedded {
    config: AppConfig,
    ctx: GfxContext,
    surface: WindowSurface,
    compositor: Compositor,
    source: FrameSource,
    life: Lifecycle,
    worker: GuestWorker,
    pace_stats: Arc<Mutex<PaceStats>>,
    _audio: AudioOut,
    exit: Option<GuestExit>,
    keyboard: Option<input::backend::keyboard::KeyboardMouse>,
}

impl Embedded {
    /// Creates the device on `layer` (`size` in pixels), and starts
    /// `guest` on a guest thread with a [`GuestHost`] built from `config`.
    ///
    /// # Safety
    /// `layer` must be a valid host surface ([`host_surface::NAME`]: a
    /// `CAMetalLayer*` on Apple platforms, a XAML `SwapChainPanel` on
    /// Windows) that outlives the returned value; this must run on the main
    /// (UI) thread.
    pub unsafe fn start(
        config: AppConfig,
        layer: *mut std::ffi::c_void,
        size: (u32, u32),
        guest: GuestFn,
    ) -> anyhow::Result<Self> {
        if layer.is_null() {
            anyhow::bail!("no {}", host_surface::NAME);
        }
        let instance = host_surface::instance();
        // SAFETY: the caller guarantees the layer and its lifetime.
        let surface = unsafe { instance.create_surface_unsafe(host_surface::target(layer)) }?;
        let ctx = pollster::block_on(GfxContext::new(instance, Some(&surface)))?;
        let surface = WindowSurface::new(&ctx, surface, size, config.fps.vsync(config.vsync))?;
        let info = ctx.adapter.get_info();
        eprintln!(
            "app: {} ({:?}), surface {:?} {}x{}, features {:?}",
            info.name, info.backend, surface.config.format, size.0, size.1, ctx.caps
        );
        // What the display side runs with, once: the present mode asked
        // for and what the surface offers (AutoVsync is Fifo where offered),
        // the swap chain's frame latency, and the CPUs this process may use.
        eprintln!(
            "app: present mode {:?} (surface offers {:?}), max frame latency {}, alpha {:?}, {} logical CPUs",
            surface.config.present_mode,
            caps_present_modes(&ctx, &surface),
            surface.config.desired_maximum_frame_latency,
            surface.config.alpha_mode,
            std::thread::available_parallelism().map_or(0, |n| n.get())
        );
        let (sink, source) = frame::channel(&ctx);
        let life = Lifecycle::new();
        // As in `h2 play`, the guest's DirectSound opens the device itself;
        // the shell's mixer only needs consuming.
        let audio = AudioOut::silent(FALLBACK_RATE, Some(life.clone()));
        let (input, keyboard) = crate::build_input(&config);
        let kept_keyboard = keyboard.clone();
        let pace_stats = Arc::default();
        let host = GuestHost {
            gfx: crate::share_context(&ctx),
            sink,
            input: Some(input),
            keyboard,
            mixer: Arc::clone(audio.mixer()),
            sample_rate: audio.sample_rate(),
            life: life.clone(),
            pacer: Pacer::new(config.pacing),
            epoch: Instant::now(),
            pace_stats: Arc::clone(&pace_stats),
            _source: None,
            _discard: None,
        };
        let worker = GuestWorker::spawn(host, guest, || {})?;
        Ok(Self {
            compositor: Compositor::new(&ctx.device),
            config,
            ctx,
            surface,
            source,
            life,
            worker,
            pace_stats,
            _audio: audio,
            exit: None,
            keyboard: kept_keyboard,
        })
    }

    /// Draws the newest guest frame, letterboxed as `config` says. Call it
    /// from the display link.
    pub fn redraw(&mut self) {
        // The host perf line (timeline::host): this thread's CPU time and
        // where it waits.
        timeline::host::sample_thread(timeline::host::Role::Ui);
        let frame = self.source.latest();
        let t = timeline::host::enabled().then(Instant::now);
        let target = self.surface.acquire(&self.ctx);
        if let Some(t) = t {
            let ns = t.elapsed().as_nanos() as u64;
            timeline::host::ACQUIRE.record(ns);
            if target.is_none() {
                timeline::host::ACQUIRE_MISSED.record(ns);
            }
        }
        let Some(target) = target else {
            return;
        };
        let view = target.texture.create_view(&Default::default());
        let viewport = frame
            .as_ref()
            .map(|f| {
                scale::place(
                    f.size,
                    self.config.aspect,
                    self.config.scaling,
                    self.surface.size(),
                )
            })
            .unwrap_or_default();
        let mut enc = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("embedded present"),
            });
        self.compositor.draw(
            &self.ctx.device,
            &mut enc,
            frame.as_ref().map(|f| &f.view),
            &view,
            self.surface.config.format,
            viewport,
            self.config.filter,
            wgpu::Color::BLACK,
        );
        let cmd = enc.finish();
        timeline::host::UI_SUBMIT.time(|| self.ctx.queue.submit([cmd]));
        timeline::host::PRESENT.time(|| self.ctx.queue.present(target));
    }

    /// The player-one pad's handle: whether the game's virtual keyboard has
    /// a field open ([`KeyboardMouse::text_active`]) and where the host's
    /// committed text goes ([`KeyboardMouse::commit_text`]).
    ///
    /// [`KeyboardMouse::text_active`]: input::backend::keyboard::KeyboardMouse::text_active
    /// [`KeyboardMouse::commit_text`]: input::backend::keyboard::KeyboardMouse::commit_text
    pub fn keyboard(&self) -> Option<&input::backend::keyboard::KeyboardMouse> {
        self.keyboard.as_ref()
    }

    /// The layer's drawable size changed (pixels).
    pub fn resize(&mut self, size: (u32, u32)) {
        self.surface.resize(size);
    }

    /// Pauses or resumes the guest (the app left or regained the screen).
    pub fn set_paused(&self, paused: bool) {
        self.life.set_paused(paused);
    }

    /// How the guest thread ended, once it has.
    pub fn guest_exit(&mut self) -> Option<&GuestExit> {
        if self.exit.is_none() {
            self.exit = self.worker.try_finish().cloned();
        }
        self.exit.as_ref()
    }

    /// Stops the guest (waiting up to `config.shutdown_timeout`) and
    /// reports as [`crate::run`] does.
    pub fn stop(mut self) -> ExitReport {
        let guest = match self.guest_exit().cloned() {
            Some(e) => e,
            None => self.worker.stop(self.config.shutdown_timeout),
        };
        ExitReport {
            guest,
            frames: self.source.stats(),
            pacing: *self.pace_stats.lock().unwrap_or_else(|e| e.into_inner()),
            restart_requested: false,
        }
    }
}

/// The present modes the host's surface offers.
fn caps_present_modes(ctx: &GfxContext, surface: &WindowSurface) -> Vec<wgpu::PresentMode> {
    surface.surface.get_capabilities(&ctx.adapter).present_modes
}

/// The shell settings a tvOS or iOS host starts from: full screen with the
/// 4:3 guest picture fitted, pads (the remote, the on-screen controls) on,
/// no mouse, and `h2 play`'s shutdown grace. `keyboard` stays on for the
/// player-one pad that merges the devices and records the one last used,
/// and for the host's text in the game's virtual keyboard.
pub fn device_config() -> AppConfig {
    AppConfig {
        title: "Halo 2".into(),
        fullscreen: true,
        keyboard: true,
        mouse_capture: false,
        gamepads: true,
        audio: false,
        shutdown_timeout: Duration::from_secs(10),
        ..AppConfig::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_config_keeps_pads_and_the_player_one_pad() {
        let c = device_config();
        assert!(c.keyboard && !c.mouse_capture && c.gamepads && c.fullscreen);
        // The player-one pad tracks the last-used device for the icons.
        let (_, keyboard) = crate::build_input(&AppConfig {
            gamepads: false,
            ..c.clone()
        });
        assert!(keyboard.is_some());
        assert!(!c.audio, "boot's DirectSound owns the device");
        assert_eq!(c.aspect, crate::DisplayAspect::Standard);
        assert_eq!(c.scaling, crate::Scaling::Fit);
    }

    #[test]
    fn windows_hosts_use_dx12_alone() {
        // No GL on Windows: opengl32.dll is not in the Xbox app container.
        let b = host_surface::backends();
        if cfg!(windows) {
            assert_eq!(b, wgpu::Backends::DX12);
        } else {
            assert_eq!(b, wgpu::Backends::all());
        }
    }

    #[test]
    fn a_null_layer_is_refused_before_any_device_work() {
        // SAFETY: null is checked first and nothing else is touched.
        let r = unsafe {
            Embedded::start(
                device_config(),
                std::ptr::null_mut(),
                (1920, 1080),
                Box::new(|_| Ok(())),
            )
        };
        assert!(r.is_err());
    }
}
