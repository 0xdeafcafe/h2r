//! The winit side: window, surface, event routing, guest thread.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gfx::GfxContext;
use input::backend::keyboard::KeyboardMouse;
use input::bindings::MouseButton;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::PhysicalKey;
use winit::window::{CursorGrabMode, Fullscreen, Window, WindowId};

use crate::audio_out::{AudioOut, FALLBACK_RATE};
use crate::frame::{self, Compositor, FrameSource, FrameStats};
use crate::keymap::{self, Shortcut};
use crate::lifecycle::Lifecycle;
use crate::pacing::{PaceStats, Pacer, WindowedRate};
use crate::scale;
use crate::worker::{GuestFn, GuestWorker};
use crate::{AppConfig, ExitReport, GuestExit, GuestHost};

#[derive(Debug)]
enum UserEvent {
    /// A frame was published since the last wake.
    Frame,
    /// The guest thread finished.
    GuestExited,
}

/// The window's swapchain. gfx's `Presenter` always uses `AutoVsync` and
/// allocates a depth buffer; the shell needs neither restriction.
struct WindowSurface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
}

impl WindowSurface {
    fn new(
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
        Ok(Self { surface, config })
    }

    fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    fn resize(&mut self, ctx: &GfxContext, size: (u32, u32)) {
        if size.0 == 0 || size.1 == 0 || size == self.size() {
            return;
        }
        self.config.width = size.0;
        self.config.height = size.1;
        self.surface.configure(&ctx.device, &self.config);
    }

    fn acquire(&mut self, ctx: &GfxContext) -> Option<wgpu::SurfaceTexture> {
        use wgpu::CurrentSurfaceTexture as C;
        match self.surface.get_current_texture() {
            C::Success(t) | C::Suboptimal(t) => Some(t),
            C::Outdated | C::Lost => {
                self.surface.configure(&ctx.device, &self.config);
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

struct Gpu {
    window: Arc<Window>,
    ctx: GfxContext,
    surface: WindowSurface,
    compositor: Compositor,
    source: FrameSource,
}

/// The title's rates average this long: the guest can present slower than
/// once a second, where a one-second window reads 0.
const STATUS_WINDOW: Duration = Duration::from_secs(5);

struct Status {
    last: Instant,
    guest: WindowedRate,
    shown: WindowedRate,
    dropped: WindowedRate,
    epoch: Instant,
}

struct Shell {
    config: AppConfig,
    guest: Option<GuestFn>,
    proxy: EventLoopProxy<UserEvent>,
    gpu: Option<Gpu>,
    keyboard: Option<KeyboardMouse>,
    audio: Option<AudioOut>,
    life: Lifecycle,
    worker: Option<GuestWorker>,
    exit: Option<GuestExit>,
    pace_stats: Arc<Mutex<PaceStats>>,
    init_error: Option<anyhow::Error>,
    captured: bool,
    super_held: bool,
    user_paused: bool,
    focus_paused: bool,
    closing: bool,
    status: Status,
    frames_at_exit: FrameStats,
}

pub(crate) fn run(config: AppConfig, guest: GuestFn) -> anyhow::Result<ExitReport> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let now = Instant::now();
    let mut shell = Shell {
        proxy: event_loop.create_proxy(),
        config,
        guest: Some(guest),
        gpu: None,
        keyboard: None,
        audio: None,
        life: Lifecycle::new(),
        worker: None,
        exit: None,
        pace_stats: Arc::default(),
        init_error: None,
        captured: false,
        super_held: false,
        user_paused: false,
        focus_paused: false,
        closing: false,
        status: Status {
            last: now,
            guest: WindowedRate::new(STATUS_WINDOW),
            shown: WindowedRate::new(STATUS_WINDOW),
            dropped: WindowedRate::new(STATUS_WINDOW),
            epoch: now,
        },
        frames_at_exit: FrameStats::default(),
    };
    event_loop.run_app(&mut shell)?;
    if let Some(e) = shell.init_error.take() {
        return Err(e);
    }
    shell.stop_guest();
    if let Some(g) = &shell.gpu {
        shell.frames_at_exit = g.source.stats();
    }
    Ok(ExitReport {
        guest: shell.exit.take().unwrap_or(GuestExit::NotStarted),
        frames: shell.frames_at_exit,
        pacing: *shell.pace_stats.lock().unwrap_or_else(|e| e.into_inner()),
    })
}

impl Shell {
    fn init(&mut self, event_loop: &ActiveEventLoop) -> anyhow::Result<()> {
        let (w, h) = self.config.window_size;
        let mut attrs = Window::default_attributes()
            .with_title(self.config.title.clone())
            .with_inner_size(LogicalSize::new(w.max(160), h.max(120)))
            .with_min_inner_size(LogicalSize::new(160, 120));
        if self.config.fullscreen {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        let window = Arc::new(event_loop.create_window(attrs)?);
        let size = window.inner_size();
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(Arc::clone(&window))?;
        let ctx = pollster::block_on(GfxContext::new(instance, Some(&surface)))?;
        let surface =
            WindowSurface::new(&ctx, surface, (size.width, size.height), self.config.vsync)?;
        let info = ctx.adapter.get_info();
        eprintln!(
            "app: {} ({:?}), surface {:?}",
            info.name, info.backend, surface.config.format
        );

        let (sink, source) = frame::channel(&ctx);
        let proxy = self.proxy.clone();
        source.set_waker(move || {
            let _ = proxy.send_event(UserEvent::Frame);
        });

        let audio = if self.config.audio {
            match AudioOut::start_default() {
                Ok(a) => {
                    eprintln!(
                        "app: audio on {} at {} Hz",
                        a.device_name(),
                        a.sample_rate()
                    );
                    a
                }
                Err(e) => {
                    eprintln!("app: no audio output ({e}); the guest mixes into a silent mixer");
                    AudioOut::silent(FALLBACK_RATE, Some(self.life.clone()))
                }
            }
        } else {
            AudioOut::silent(FALLBACK_RATE, Some(self.life.clone()))
        };

        let (input, keyboard) = crate::build_input(&self.config);
        self.keyboard = keyboard.clone();
        let host = GuestHost {
            gfx: crate::share_context(&ctx),
            sink,
            input: Some(input),
            keyboard,
            mixer: Arc::clone(audio.mixer()),
            sample_rate: audio.sample_rate(),
            life: self.life.clone(),
            pacer: Pacer::new(self.config.pacing),
            epoch: Instant::now(),
            pace_stats: Arc::clone(&self.pace_stats),
            _source: None,
            _discard: None,
        };
        self.audio = Some(audio);

        let guest = self.guest.take().expect("init runs once");
        let proxy = self.proxy.clone();
        self.worker = Some(GuestWorker::spawn(host, guest, move || {
            let _ = proxy.send_event(UserEvent::GuestExited);
        })?);

        window.request_redraw();
        self.gpu = Some(Gpu {
            window,
            compositor: Compositor::new(&ctx.device),
            ctx,
            surface,
            source,
        });
        Ok(())
    }

    fn redraw(&mut self) {
        let Some(g) = self.gpu.as_mut() else { return };
        let frame = g.source.latest();
        let Some(target) = g.surface.acquire(&g.ctx) else {
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
                    g.surface.size(),
                )
            })
            .unwrap_or_default();
        let mut enc = g
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("app present"),
            });
        g.compositor.draw(
            &g.ctx.device,
            &mut enc,
            frame.as_ref().map(|f| &f.view),
            &view,
            g.surface.config.format,
            viewport,
            self.config.filter,
            wgpu::Color::BLACK,
        );
        g.ctx.queue.submit([enc.finish()]);
        g.window.pre_present_notify();
        g.ctx.queue.present(target);
    }

    fn update_status(&mut self) {
        let Some(g) = self.gpu.as_ref() else { return };
        let dt = self.status.last.elapsed();
        if dt < Duration::from_secs(1) {
            return;
        }
        let stats = g.source.stats();
        let now = self.status.epoch.elapsed();
        let guest_fps = self.status.guest.record(now, stats.published);
        let shown_fps = self.status.shown.record(now, stats.shown);
        self.status.dropped.record(now, stats.dropped);
        self.status.last = Instant::now();
        let mut line = format!(
            "{} | guest {:.1} fps | shown {:.1} fps | dropped {} (last {}s)",
            self.config.title,
            guest_fps,
            shown_fps,
            self.status.dropped.count(),
            STATUS_WINDOW.as_secs()
        );
        if self.life.is_paused() {
            line.push_str(" | paused");
        }
        if let Some(e) = &self.exit {
            line.push_str(&format!(" | guest exited: {}", describe(e)));
        }
        if self.config.log_status {
            eprintln!("{line}");
        }
        g.window.set_title(&line);
    }

    fn capture(&mut self, on: bool) {
        let Some(g) = self.gpu.as_ref() else { return };
        if on == self.captured {
            return;
        }
        if on {
            let grabbed = g
                .window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| g.window.set_cursor_grab(CursorGrabMode::Confined));
            if let Err(e) = grabbed {
                eprintln!("app: mouse capture unavailable: {e}");
                return;
            }
            g.window.set_cursor_visible(false);
        } else {
            let _ = g.window.set_cursor_grab(CursorGrabMode::None);
            g.window.set_cursor_visible(true);
            if let Some(kb) = &self.keyboard {
                for b in [
                    MouseButton::Left,
                    MouseButton::Right,
                    MouseButton::Middle,
                    MouseButton::Back,
                    MouseButton::Forward,
                ] {
                    kb.mouse_button(b, false);
                }
            }
        }
        self.captured = on;
    }

    fn apply_pause(&mut self) {
        let paused = self.user_paused || self.focus_paused;
        if paused != self.life.is_paused() {
            self.life.set_paused(paused);
            if let Some(a) = &self.audio {
                if paused { a.pause() } else { a.resume() }
            }
        }
    }

    fn toggle_fullscreen(&self) {
        if let Some(g) = &self.gpu {
            let next = match g.window.fullscreen() {
                Some(_) => None,
                None => Some(Fullscreen::Borderless(None)),
            };
            g.window.set_fullscreen(next);
        }
    }

    /// Ask the guest to stop and wait up to `shutdown_timeout` for it.
    fn stop_guest(&mut self) {
        let Some(w) = self.worker.as_mut() else {
            return;
        };
        let e = w.stop(self.config.shutdown_timeout);
        if e == GuestExit::StillRunning {
            eprintln!(
                "app: guest did not stop within {:?}; leaving it to process exit",
                self.config.shutdown_timeout
            );
        }
        self.exit = Some(e);
    }

    fn close(&mut self, event_loop: &ActiveEventLoop, why: &str) {
        if self.closing {
            return;
        }
        eprintln!("app: closing ({why})");
        self.closing = true;
        self.capture(false);
        if let Some(a) = &self.audio {
            a.pause();
        }
        self.stop_guest();
        if let Some(g) = &self.gpu {
            self.frames_at_exit = g.source.stats();
        }
        event_loop.exit();
    }
}

fn describe(e: &GuestExit) -> String {
    match e {
        GuestExit::Returned => "ok".into(),
        GuestExit::Failed(m) | GuestExit::Panicked(m) => {
            m.lines().next().unwrap_or("").chars().take(120).collect()
        }
        GuestExit::StillRunning => "still running".into(),
        GuestExit::NotStarted => "not started".into(),
    }
}

impl ApplicationHandler<UserEvent> for Shell {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() || self.init_error.is_some() {
            return;
        }
        if let Err(e) = self.init(event_loop) {
            self.init_error = Some(e);
            event_loop.exit();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Frame => {
                if let Some(g) = &self.gpu {
                    g.source.ack_wake();
                    g.window.request_redraw();
                }
            }
            UserEvent::GuestExited => {
                if let Some(e) = self.worker.as_mut().and_then(|w| w.try_finish()).cloned() {
                    eprintln!("app: guest thread exited: {e:?}");
                    self.exit = Some(e);
                }
                if self.config.keep_open_on_guest_exit {
                    self.status.last = Instant::now() - Duration::from_secs(2);
                    self.update_status();
                } else {
                    self.close(event_loop, "the guest exited");
                }
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.close(event_loop, "window closed"),
            WindowEvent::Resized(size) => {
                if let Some(g) = self.gpu.as_mut() {
                    g.surface.resize(&g.ctx, (size.width, size.height));
                    g.window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Focused(focused) => {
                if !focused {
                    if let Some(kb) = &self.keyboard {
                        kb.release_all();
                    }
                    self.capture(false);
                }
                self.focus_paused = self.config.pause_on_focus_loss && !focused;
                self.apply_pause();
            }
            WindowEvent::ModifiersChanged(m) => {
                self.super_held = m.state().super_key();
                if self.super_held {
                    self.capture(false);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                let down = event.state == ElementState::Pressed;
                if self.super_held {
                    if down && !event.repeat {
                        match keymap::shortcut(code) {
                            Some(Shortcut::Quit) => self.close(event_loop, "Cmd+Q"),
                            Some(Shortcut::Fullscreen) => self.toggle_fullscreen(),
                            Some(Shortcut::Pause) => {
                                self.user_paused = !self.user_paused;
                                self.apply_pause();
                            }
                            None => {}
                        }
                    }
                    if down {
                        return;
                    }
                }
                if let (Some(kb), Some(name)) = (&self.keyboard, keymap::key_name(code)) {
                    if down {
                        kb.key_down(name);
                    } else {
                        kb.key_up(name);
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let down = state == ElementState::Pressed;
                let action = keymap::mouse_action(keymap::MouseContext {
                    capture_enabled: self.config.mouse_capture,
                    captured: self.captured,
                    super_held: self.super_held,
                    down,
                    left: button == winit::event::MouseButton::Left,
                });
                match action {
                    keymap::MouseAction::Capture => self.capture(true),
                    keymap::MouseAction::Forward => {
                        if let (Some(kb), Some(b)) = (&self.keyboard, keymap::mouse_button(button))
                        {
                            kb.mouse_button(b, down);
                        }
                    }
                    keymap::MouseAction::Ignore => {}
                }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event
            && self.captured
            && let Some(kb) = &self.keyboard
        {
            kb.mouse_motion(delta.0, delta.1);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.update_status();
        if !self.closing {
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                self.status.last + Duration::from_secs(1),
            ));
        }
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        self.life.request_stop();
    }
}
