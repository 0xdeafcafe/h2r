//! The shell for a native host that owns the window and the run loop
//! (the tvOS app, platform/tvos; docs/tvos.md), in place of winit's.
//!
//! The host hands over a `CAMetalLayer` and calls [`Embedded::redraw`] from
//! its display link on the main thread; everything else is the windowed
//! shell's: the same [`GuestHost`] on a guest thread, the same frame
//! handoff, compositor and pacing. Input is GameController: pads and the
//! Apple TV remote, behind the shell's usual player-one pad (no keyboard
//! or mouse events ever arrive; the pad only tracks the device last used,
//! for the button icons).

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
}

impl Embedded {
    /// Creates the device on `layer` (`size` in pixels), and starts
    /// `guest` on a guest thread with a [`GuestHost`] built from `config`.
    ///
    /// # Safety
    /// `layer` must be a valid `CAMetalLayer*` that outlives the returned
    /// value; this must run on the main thread.
    pub unsafe fn start(
        config: AppConfig,
        layer: *mut std::ffi::c_void,
        size: (u32, u32),
        guest: GuestFn,
    ) -> anyhow::Result<Self> {
        if layer.is_null() {
            anyhow::bail!("no CAMetalLayer");
        }
        let instance = wgpu::Instance::default();
        // SAFETY: the caller guarantees the layer and its lifetime.
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(layer))
        }?;
        let ctx = pollster::block_on(GfxContext::new(instance, Some(&surface)))?;
        let surface = WindowSurface::new(&ctx, surface, size, config.fps.vsync(config.vsync))?;
        let info = ctx.adapter.get_info();
        eprintln!(
            "app: {} ({:?}), surface {:?} {}x{}, features {:?}",
            info.name, info.backend, surface.config.format, size.0, size.1, ctx.caps
        );
        let (sink, source) = frame::channel(&ctx);
        let life = Lifecycle::new();
        // As in `h2 play`, the guest's DirectSound opens the device itself;
        // the shell's mixer only needs consuming.
        let audio = AudioOut::silent(FALLBACK_RATE, Some(life.clone()));
        let (input, keyboard) = crate::build_input(&config);
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
        })
    }

    /// Draws the newest guest frame, letterboxed as `config` says. Call it
    /// from the display link.
    pub fn redraw(&mut self) {
        let frame = self.source.latest();
        let Some(target) = self.surface.acquire(&self.ctx) else {
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
        self.ctx.queue.submit([enc.finish()]);
        self.ctx.queue.present(target);
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

/// The shell settings a tvOS host starts from: full screen with the 4:3
/// guest picture fitted, pads and the remote on, no mouse, and `h2 play`'s
/// shutdown grace. `keyboard` stays on for the player-one pad that merges
/// the devices and records the one last used (no key ever arrives).
pub fn tv_config() -> AppConfig {
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
    fn tv_config_keeps_pads_and_the_player_one_pad() {
        let c = tv_config();
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
    fn a_null_layer_is_refused_before_any_device_work() {
        // SAFETY: null is checked first and nothing else is touched.
        let r = unsafe {
            Embedded::start(
                tv_config(),
                std::ptr::null_mut(),
                (1920, 1080),
                Box::new(|_| Ok(())),
            )
        };
        assert!(r.is_err());
    }
}
