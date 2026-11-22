//! The winit side: window, surface, event routing, guest thread.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gfx::GfxContext;
use input::backend::keyboard::KeyboardMouse;
use input::bindings::MouseButton;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{DeviceEvent, DeviceId, ElementState, Ime, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Fullscreen, Window, WindowId};

use crate::audio_out::{AudioOut, FALLBACK_RATE};
use crate::forge_overlay;
use crate::fps::DisplaySchedule;
use crate::frame::{self, Compositor, FrameSource, FrameStats};
use crate::keymap::{self, Shortcut};
use crate::lifecycle::Lifecycle;
use crate::menu_overlay::MenuRenderer;
use crate::overlay::marks::{MarkFrameStatus, MarkRenderer};
use crate::overlay::{EditorPlateRenderer, OverlayRenderer};
use crate::pacing::{PaceStats, Pacer, WindowedRate};
use crate::surface::{SurfaceKind, WindowSurface};
use crate::theater_overlay;
use crate::watermark::{LiveStats, Panels, WatermarkRenderer};
use crate::worker::{GuestFn, GuestWorker};
use crate::{AppConfig, ExitReport, GuestExit, GuestHost};

#[derive(Debug)]
enum UserEvent {
    /// A frame was published since the last wake.
    Frame,
    /// The guest thread finished.
    GuestExited,
    /// The editor changed its plate or asked for focus.
    EditorChanged,
    /// A [`crate::Closer`] asked to close.
    Close,
}

struct Gpu {
    window: Arc<Window>,
    ctx: GfxContext,
    surface: WindowSurface,
    compositor: Compositor,
    overlay: OverlayRenderer,
    editor: Option<EditorPlateRenderer>,
    marks: MarkRenderer,
    watermark: WatermarkRenderer,
    menu: MenuRenderer,
    theater: Option<theater_overlay::Renderer>,
    /// The adapter's backend, lower case (`metal`), for the watermark.
    backend: String,
    source: FrameSource,
    #[cfg(target_os = "macos")]
    metal_resize: Option<crate::metal_resize::MetalResize>,
    #[cfg(target_os = "macos")]
    last_presented_size: (u32, u32),
}

/// How often the watermark's numbers change.
const WATERMARK_REFRESH: Duration = Duration::from_millis(250);

/// The watermark's text and when it was made.
struct WatermarkText {
    at: Instant,
    /// Presents published by then, for the fps without frame stats.
    published: u64,
    panels: Panels,
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
    shift_held: bool,
    forge_modal: bool,
    restore_forge_capture: bool,
    ime_composing: bool,
    ime_allowed: bool,
    #[cfg(target_os = "linux")]
    steam_keyboard: Option<crate::steam_keyboard::SteamKeyboard>,
    user_paused: bool,
    focus_paused: bool,
    closing: bool,
    status: Status,
    frames_at_exit: FrameStats,
    /// The banner generation last drawn.
    banner_drawn: u64,
    editor_redraw: EditorRedraw,
    mark_frame_status: Option<MarkFrameStatus>,
    restart_requested: bool,
    watermark_text: Option<WatermarkText>,
    theater_view: theater_overlay::View,
    theater_refresh: Instant,
    theater_screenshot: Option<std::path::PathBuf>,
    theater_cursor: Option<(f64, f64)>,
    /// The window's own redraw clock above 30 fps (`AppConfig::fps`).
    display: DisplaySchedule,
}

/// Keep an expired or changed overlay dirty until a replacement presents.
#[derive(Default)]
struct EditorRedraw {
    expires: Option<Instant>,
    changed: bool,
    retry_at: Option<Instant>,
}

const EDITOR_REDRAW_RETRY: Duration = Duration::from_millis(100);

impl EditorRedraw {
    fn change(&mut self) {
        self.changed = true;
    }

    fn presented(&mut self, expires: Option<Instant>) {
        self.expires = expires;
        self.changed = false;
        self.retry_at = None;
    }

    fn schedule(&mut self, now: Instant) -> (bool, Option<Instant>) {
        if !self.changed && self.expires.is_none_or(|expires| now < expires) {
            return (false, self.expires);
        }
        if let Some(retry) = self.retry_at
            && now < retry
        {
            return (false, Some(retry));
        }
        let retry = now + EDITOR_REDRAW_RETRY;
        self.retry_at = Some(retry);
        (true, Some(retry))
    }
}

pub(crate) fn run(config: AppConfig, guest: GuestFn) -> anyhow::Result<ExitReport> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let now = Instant::now();
    let config_fps = config.fps;
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
        shift_held: false,
        forge_modal: false,
        restore_forge_capture: false,
        ime_composing: false,
        ime_allowed: false,
        #[cfg(target_os = "linux")]
        steam_keyboard: crate::steam_keyboard::SteamKeyboard::from_env(),
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
        banner_drawn: 0,
        editor_redraw: EditorRedraw::default(),
        mark_frame_status: None,
        restart_requested: false,
        watermark_text: None,
        theater_view: theater_overlay::View::default(),
        theater_refresh: now,
        theater_screenshot: None,
        theater_cursor: None,
        display: DisplaySchedule::new(config_fps),
    };
    if let Some(c) = &shell.config.closer {
        let proxy = shell.proxy.clone();
        c.attach(
            shell.life.clone(),
            Box::new(move || {
                let _ = proxy.send_event(UserEvent::Close);
            }),
        );
    }
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
        restart_requested: shell.restart_requested,
    })
}

impl Shell {
    fn init(&mut self, event_loop: &ActiveEventLoop) -> anyhow::Result<()> {
        let (w, h) = self.config.window_size;
        let mut attrs = Window::default_attributes()
            .with_title(self.config.title.clone())
            .with_inner_size(LogicalSize::new(w.max(160), h.max(120)))
            .with_min_inner_size(LogicalSize::new(160, 120));
        if let Some((index, count)) = self.config.tile
            && let Some(monitor) = event_loop.primary_monitor()
        {
            let size = monitor.size().to_logical::<f64>(monitor.scale_factor());
            if let Some((x, y, w, h)) =
                crate::tile::tile((size.width, size.height), 40.0, index, count)
            {
                attrs = attrs
                    .with_position(winit::dpi::LogicalPosition::new(x, y))
                    .with_inner_size(LogicalSize::new(w, h));
            }
        }
        if self.config.fullscreen {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        let window = Arc::new(event_loop.create_window(attrs)?);
        // No bundle carries an .icns, so the Dock shows a generic icon
        // unless the app sets one (art/app-icon).
        #[cfg(target_os = "macos")]
        app_icon::set_dock_icon();
        let size = window.inner_size();
        let instance = gfx::instance();
        let surface = instance.create_surface(Arc::clone(&window))?;
        let ctx = pollster::block_on(GfxContext::new(instance, Some(&surface)))?;
        let info = ctx.adapter.get_info();
        #[cfg(target_os = "macos")]
        let metal_resize = if info.backend == wgpu::Backend::Metal {
            crate::metal_resize::MetalResize::new(&window, &surface)
        } else {
            None
        };
        #[cfg(target_os = "macos")]
        let size = metal_resize
            .as_ref()
            .map_or(size, |metal| metal.drawable_size());
        let surface = WindowSurface::new(
            &ctx,
            surface,
            (size.width, size.height),
            &self.config,
            SurfaceKind::Window,
        )?;
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
            native_size: surface.native_size.clone(),
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

        if let Some(editor) = &self.config.editor_link {
            let proxy = self.proxy.clone();
            editor.set_waker(Arc::new(move || {
                let _ = proxy.send_event(UserEvent::EditorChanged);
            }));
        }
        let guest = self.guest.take().expect("init runs once");
        let proxy = self.proxy.clone();
        self.worker = Some(GuestWorker::spawn(host, guest, move || {
            let _ = proxy.send_event(UserEvent::GuestExited);
        })?);

        window.request_redraw();
        self.gpu = Some(Gpu {
            window,
            compositor: Compositor::new(&ctx.device),
            overlay: OverlayRenderer::new(&ctx.device),
            editor: self
                .config
                .editor_link
                .as_ref()
                .map(|_| EditorPlateRenderer::new(&ctx.device)),
            marks: MarkRenderer::default(),
            watermark: WatermarkRenderer::new(&ctx.device),
            menu: MenuRenderer::new(&ctx.device),
            theater: (self.config.theater.is_some() || self.config.forge.is_some())
                .then(|| theater_overlay::Renderer::new(&ctx.device)),
            backend: format!("{:?}", info.backend).to_lowercase(),
            ctx,
            surface,
            source,
            #[cfg(target_os = "macos")]
            metal_resize,
            #[cfg(target_os = "macos")]
            last_presented_size: (0, 0),
        });
        Ok(())
    }

    fn redraw(&mut self, size: Option<PhysicalSize<u32>>) {
        let forge_frame = self.config.forge.as_ref().and_then(|forge| forge.0.frame());
        let modal = forge_frame.as_ref().is_some_and(|frame| frame.modal);
        if modal && !self.forge_modal {
            self.restore_forge_capture = self.captured;
            self.capture(false);
        } else if !modal && self.forge_modal {
            if forge_frame.is_some() && self.restore_forge_capture {
                self.capture(true);
            }
            self.restore_forge_capture = false;
        }
        self.forge_modal = modal;
        let Some(g) = self.gpu.as_mut() else { return };
        // AppKit can call drawRect before delivering Resized. A queued
        // resize event can also refer to an older size than the live view.
        #[cfg(target_os = "macos")]
        let size = g
            .metal_resize
            .as_ref()
            .map(|metal| metal.drawable_size())
            .or(size);
        if let Some(size) = size {
            if size.width == 0 || size.height == 0 {
                return;
            }
            g.surface.resize((size.width, size.height));
        }
        let size = g.surface.size();
        #[cfg(target_os = "macos")]
        let transaction = g
            .metal_resize
            .as_ref()
            .and_then(|metal| metal.transaction(size != g.last_presented_size));
        let Some(target) = g.surface.acquire(&g.ctx) else {
            if g.surface.needs_configure() {
                g.window.request_redraw();
            }
            return;
        };
        let frame = g.source.latest();
        let view = target.texture.create_view(&Default::default());
        let viewport = g.surface.viewport(frame.as_ref());
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
        let now = Instant::now();
        let mut overlay_expires = None;
        let mut mark_status = MarkFrameStatus::Waiting;
        let probing = gfx::world_lines::probe::PRESENTED.armed();
        let mut anchors = Vec::new();
        if let Some(frame) = &frame {
            if frame.loading {
                mark_status = MarkFrameStatus::Loading;
            } else {
                let mut drawn_generation = None;
                if let Some(marks) = frame.marks.marks() {
                    g.marks.begin();
                    let shown = marks.visit(now, &mut |cameras, mark| {
                        if probing && anchors.len() < gfx::world_lines::probe::ANCHORS {
                            anchors.push(crate::overlay::marks::anchor_pixels(
                                cameras,
                                viewport,
                                mark.anchor,
                            ));
                        }
                        g.marks.push(&mark, cameras, viewport, g.surface.size());
                    });
                    marks.visit_hud(now, &mut |mark| {
                        g.marks.push_hud(&mark, viewport, g.surface.size());
                    });
                    overlay_expires = shown.expires;
                    drawn_generation = shown.editor_generation;
                    g.marks.draw(
                        &g.ctx.device,
                        &g.ctx.queue,
                        &mut enc,
                        &view,
                        g.surface.config.format,
                    );
                }
                mark_status = frame.marks.status(drawn_generation);
            }
        }
        if probing {
            crate::overlay::marks::record_probe(
                frame.as_ref().map(|f| &f.marks),
                frame.as_ref().is_some_and(|f| f.loading),
                anchors,
                now,
            );
        }
        if frame.as_ref().is_some_and(|f| f.loading) {
            self.watermark_text = None;
        } else if let Some(wm) = &self.config.watermark
            && wm.visible()
        {
            let published = g.source.stats().published;
            let stale = self
                .watermark_text
                .as_ref()
                .is_none_or(|t| t.at.elapsed() >= WATERMARK_REFRESH);
            if stale {
                let stats = timeline::phase::window_summary()
                    .map(|s| LiveStats::from_summary(&s))
                    .or_else(|| {
                        let t = self.watermark_text.as_ref()?;
                        LiveStats::from_rate(
                            published.saturating_sub(t.published),
                            t.at.elapsed().as_secs_f64(),
                        )
                    });
                self.watermark_text = Some(WatermarkText {
                    at: Instant::now(),
                    published,
                    panels: wm.panels(stats.as_ref(), &g.backend),
                });
            }
            if let Some(t) = &self.watermark_text {
                g.watermark.draw(
                    &g.ctx.device,
                    &g.ctx.queue,
                    &mut enc,
                    &t.panels,
                    &view,
                    g.surface.config.format,
                    size,
                    viewport,
                );
            }
        }
        let notice = self.config.banner.as_ref().and_then(|banner| {
            self.banner_drawn = banner.generation();
            banner.visible()
        });
        if let Some(editor) = &self.config.editor_link
            && let Some(renderer) = &mut g.editor
            && let Some((plate, expires)) = editor.current(now)
        {
            overlay_expires = Some(overlay_expires.map_or(expires, |marks| marks.min(expires)));
            renderer.draw(
                &g.ctx.device,
                &g.ctx.queue,
                &mut enc,
                &plate,
                notice.is_some(),
                &view,
                g.surface.config.format,
                g.surface.size(),
            );
        }
        if let Some(renderer) = &mut g.theater {
            let model = forge_frame.map(|frame| theater_overlay::View {
                playing: false,
                page: Some(frame.page),
            });
            renderer.draw(
                &g.ctx,
                &mut enc,
                model.as_ref().unwrap_or(&self.theater_view),
                &view,
                g.surface.config.format,
                size,
                viewport,
            );
        }
        // The debug menu over everything but the banner.
        if let Some(lines) = self
            .config
            .debug_menu
            .as_ref()
            .and_then(debug_menu::Shared::lines)
        {
            g.menu.draw(
                &g.ctx.device,
                &g.ctx.queue,
                &mut enc,
                &lines,
                &view,
                g.surface.config.format,
                size,
                viewport,
            );
        }
        if let Some(notice) = notice {
            g.overlay.draw(
                &g.ctx.device,
                &g.ctx.queue,
                &mut enc,
                &notice,
                &view,
                g.surface.config.format,
                size,
            );
        }
        g.ctx.queue.submit([enc.finish()]);
        g.window.pre_present_notify();
        g.ctx.queue.present(target);
        #[cfg(target_os = "macos")]
        {
            drop(transaction);
            g.last_presented_size = size;
        }
        self.editor_redraw.presented(overlay_expires);
        if let Some(editor) = &self.config.editor_link
            && self.mark_frame_status != Some(mark_status)
        {
            editor.set_mark_frame_status(mark_status);
            self.mark_frame_status = Some(mark_status);
        }
        if let Some(theater) = &self.config.theater
            && let Some(dir) = self.theater_screenshot.take()
        {
            theater.0.screenshot_saved(theater_overlay::screenshot(&g.ctx, frame.as_ref(), &dir));
        }
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
        if self.config.status_in_title {
            g.window.set_title(&line);
        }
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
                    if self.display.redraw_on_publish() {
                        g.window.request_redraw();
                    }
                }
            }
            UserEvent::EditorChanged => {
                self.editor_redraw.change();
                if let (Some(editor), Some(g)) = (&self.config.editor_link, &self.gpu) {
                    if editor.take_focus_request() {
                        if g.window.is_minimized() == Some(true) {
                            g.window.set_minimized(false);
                        }
                        g.window.focus_window();
                    }
                }
            }
            UserEvent::Close => self.close(event_loop, "close requested"),
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
            WindowEvent::Resized(size) => self.redraw(Some(size)),
            WindowEvent::RedrawRequested | WindowEvent::Occluded(false) => self.redraw(None),
            WindowEvent::Focused(focused) => {
                if !focused && let Some(theater) = &self.config.theater {
                    theater.0.release_keys();
                }
                if !focused {
                    self.restore_forge_capture = false;
                    if let Some(forge) = &self.config.forge {
                        forge.0.release_keys();
                    }
                    if let Some(kb) = &self.keyboard {
                        kb.release_all();
                    }
                    self.capture(false);
                    self.ime_composing = false;
                }
                self.focus_paused = self.config.pause_on_focus_loss && !focused;
                self.apply_pause();
            }
            WindowEvent::ModifiersChanged(m) => {
                self.super_held = m.state().super_key();
                self.shift_held = m.state().shift_key();
                if self.super_held {
                    self.restore_forge_capture = false;
                    if let Some(forge) = &self.config.forge {
                        forge.0.release_keys();
                    }
                    self.capture(false);
                }
            }
            WindowEvent::Ime(ime) => match ime {
                Ime::Commit(text) => {
                    if !self.super_held
                        && let Some(kb) = &self.keyboard
                    {
                        kb.commit_text(&text);
                    }
                    self.ime_composing = false;
                }
                Ime::Preedit(text, _) => self.ime_composing = !text.is_empty(),
                Ime::Disabled => self.ime_composing = false,
                Ime::Enabled => {}
            },
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    if !self.super_held
                        && let Some(kb) = &self.keyboard
                    {
                        keymap::text_input(
                            kb,
                            None,
                            &event.logical_key,
                            event.text.as_deref(),
                            event.state == ElementState::Pressed,
                            self.ime_composing,
                        );
                    }
                    return;
                };
                let down = event.state == ElementState::Pressed;
                if !self.config.debug_menu.as_ref().is_some_and(debug_menu::Shared::is_open)
                    && let Some(forge) = &self.config.forge
                    && let Some(key) = forge_overlay::key(code, self.super_held, self.shift_held)
                    && forge.0.key(key, down, event.repeat)
                {
                    if !down && let (Some(kb), Some(name)) = (&self.keyboard, keymap::key_name(code)) {
                        kb.key_up(name);
                    }
                    if let Some(g) = &self.gpu {
                        g.window.request_redraw();
                    }
                    return;
                }
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
                if let Some(wm) = &self.config.watermark
                    && code == KeyCode::F3
                {
                    if down && !event.repeat {
                        wm.toggle();
                        if let Some(g) = &self.gpu {
                            g.window.request_redraw();
                        }
                    }
                    return;
                }
                if let Some(menu) = self.config.debug_menu.clone() {
                    match keymap::menu_key(code, menu.is_open()) {
                        keymap::MenuKey::Take(k) => {
                            // The OS repeats a held arrow, page key or
                            // Backspace; the other keys act once a press.
                            if down
                                && let Some(k) = k
                                && (!event.repeat || k.repeats())
                            {
                                menu.key(k);
                                if let Some(g) = &self.gpu {
                                    g.window.request_redraw();
                                }
                            }
                            // A release still reaches the pad, so a key held
                            // when the menu opened is not stuck down.
                            if !down
                                && let (Some(kb), Some(name)) =
                                    (&self.keyboard, keymap::key_name(code))
                            {
                                kb.key_up(name);
                            }
                            return;
                        }
                        keymap::MenuKey::Pass => {}
                    }
                }
                if let Some(theater) = &self.config.theater
                    && let Some(key) = theater_overlay::key(code)
                    && theater.0.key(key, down, event.repeat)
                {
                    if !down && let (Some(kb), Some(name)) = (&self.keyboard, keymap::key_name(code)) {
                        kb.key_up(name);
                    }
                    self.theater_refresh = Instant::now();
                    if let Some(g) = &self.gpu {
                        g.window.request_redraw();
                    }
                    return;
                }
                if let Some(banner) = self.config.banner.clone()
                    && matches!(code, KeyCode::F5 | KeyCode::F6)
                {
                    if down && !event.repeat {
                        if code == KeyCode::F6 {
                            banner.dismiss();
                            if let Some(g) = &self.gpu {
                                g.window.request_redraw();
                            }
                        } else if banner.restart_offered() {
                            self.restart_requested = true;
                            self.close(event_loop, "F5: restart for update");
                        }
                    }
                    return;
                }
                if let Some(kb) = &self.keyboard
                    && keymap::text_input(
                        kb,
                        keymap::key_name(code),
                        &event.logical_key,
                        event.text.as_deref(),
                        down,
                        self.ime_composing,
                    )
                {
                    return;
                }
                if let (Some(kb), Some(name)) = (&self.keyboard, keymap::key_name(code)) {
                    if down {
                        kb.key_down(name);
                    } else {
                        kb.key_up(name);
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. }
                if self.config.theater.is_some() || self.config.forge.is_some() =>
            {
                self.theater_cursor = Some((position.x, position.y));
            }
            WindowEvent::CursorLeft { .. } => self.theater_cursor = None,
            WindowEvent::MouseWheel { delta, .. } if self.theater_view.playing => {
                let amount = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 * 0.05,
                };
                if let Some(theater) = &self.config.theater {
                    theater.0.mouse_zoom(amount);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let down = state == ElementState::Pressed;
                if !self.super_held
                    && !self.config.debug_menu.as_ref().is_some_and(debug_menu::Shared::is_open)
                    && let Some(forge) = &self.config.forge
                {
                    use winit::event::MouseButton as B;
                    if self.forge_modal {
                        if down && button == B::Left
                            && let Some((x, y)) = self.theater_cursor
                            && let Some((x, y)) = self.gpu.as_ref()
                                .and_then(|gpu| gpu.theater.as_ref())
                                .and_then(|renderer| renderer.page_point(x, y))
                        {
                            forge.0.click(x, y);
                        }
                        return;
                    }
                    if matches!(button, B::Left | B::Right)
                        && forge.0.mouse_button(button == B::Left, down)
                    {
                        if down && self.config.mouse_capture {
                            self.capture(true);
                        }
                        return;
                    }
                }
                if self.theater_view.playing
                    && !self.super_held
                    && let Some(theater) = &self.config.theater
                {
                    match button {
                        winit::event::MouseButton::Right
                            if !down || theater.0.mouse_look(0.0, 0.0) =>
                        {
                            self.capture(down);
                            return;
                        }
                        winit::event::MouseButton::Left if down && !self.captured => {
                            if let Some((x, y)) = self.theater_cursor
                                && let Some(fraction) = self.gpu.as_ref()
                                    .and_then(|gpu| gpu.theater.as_ref())
                                    .and_then(|renderer| renderer.timeline_fraction(x, y))
                                && theater.0.scrub(fraction)
                            {
                                return;
                            }
                        }
                        _ => {}
                    }
                }
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
        {
            if let Some(forge) = &self.config.forge
                && forge.0.mouse_look(delta.0 as f32, delta.1 as f32)
            {
                return;
            }
            if self.theater_view.playing && let Some(theater) = &self.config.theater {
                theater.0.mouse_look(delta.0, delta.1);
            } else if let Some(kb) = &self.keyboard {
                kb.mouse_motion(delta.0, delta.1);
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let text_active = self
            .keyboard
            .as_ref()
            .is_some_and(KeyboardMouse::text_active);
        if text_active != self.ime_allowed {
            if let Some(g) = &self.gpu {
                g.window.set_ime_allowed(text_active);
            }
            self.ime_allowed = text_active;
            #[cfg(target_os = "linux")]
            if let (Some(steam), Some(kb)) = (&mut self.steam_keyboard, &self.keyboard) {
                steam.update(text_active, kb.active_device().get());
            }
            if !text_active {
                self.ime_composing = false;
            }
        }
        self.update_status();
        // The debug menu redraws as it changes and as answers come in,
        // also while the guest presents slowly (a load).
        if let (Some(menu), Some(g)) = (&self.config.debug_menu, &self.gpu)
            && menu.needs_redraw()
        {
            g.window.request_redraw();
        }
        // A banner change shows without waiting for the guest's next present.
        if let (Some(banner), Some(g)) = (&self.config.banner, &self.gpu)
            && banner.generation() != self.banner_drawn
        {
            g.window.request_redraw();
        }
        // Fresh numbers while the guest presents slower than the refresh.
        if let (Some(wm), Some(g)) = (&self.config.watermark, &self.gpu)
            && wm.visible()
            && self
                .watermark_text
                .as_ref()
                .is_some_and(|t| t.at.elapsed() >= Duration::from_secs(1))
        {
            g.window.request_redraw();
        }
        // Above 30 fps, sample the latest guest or replay frame on the display grid.
        let mut wake = self.status.last + Duration::from_secs(1);
        if let Some(g) = &self.gpu {
            let (redraw, next) = self.editor_redraw.schedule(Instant::now());
            if redraw {
                g.window.request_redraw();
            }
            if let Some(next) = next {
                wake = wake.min(next);
            }
        }
        if let Some(theater) = &self.config.theater {
            let now = Instant::now();
            if now >= self.theater_refresh {
                let view = theater.0.update();
                if theater.0.take_leave() {
                    self.close(event_loop, "leave Theater playback");
                    return;
                }
                let rate = if view.playing { 16 } else { 150 };
                if self.theater_screenshot.is_none() {
                    self.theater_screenshot = theater.0.take_screenshot();
                }
                if self.theater_view != view || self.theater_screenshot.is_some() {
                    self.theater_view = view;
                    if let Some(g) = &self.gpu {
                        g.window.request_redraw();
                    }
                }
                self.theater_refresh = now + Duration::from_millis(rate);
            }
            wake = wake.min(self.theater_refresh);
        }
        // A new frame-rate target (the Video screen's row): the schedule
        // from now, and the surface's vsync when that changes too.
        if let Some(c) = &self.config.fps_control
            && let Some((target, vsync)) = crate::fps::follow(c, self.config.fps, self.config.vsync)
        {
            self.config.fps = target;
            self.display.retarget(target);
            if let Some(g) = self.gpu.as_mut() {
                if let Some(v) = vsync {
                    g.surface.set_vsync(v);
                }
                if self.display.redraw_on_publish() {
                    // A publish just before the switch may still be waiting for a slot.
                    g.window.request_redraw();
                }
            }
            eprintln!("app: frame rate {target}");
        }
        if let Some(g) = &self.gpu
            && !self.closing
        {
            if self.display.due(self.status.epoch.elapsed()) {
                g.window.request_redraw();
            }
            if let Some(next) = self.display.next_due() {
                wake = wake.min(self.status.epoch + next);
            }
        }
        if !self.closing {
            event_loop.set_control_flow(ControlFlow::WaitUntil(wake));
        }
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        self.life.request_stop();
    }
}

#[cfg(test)]
mod editor_redraw_tests {
    use super::*;

    #[test]
    fn expired_editor_overlay_retries_until_a_replacement_presents() {
        let start = Instant::now();
        let expires = start + Duration::from_millis(500);
        let mut redraw = EditorRedraw::default();
        redraw.presented(Some(expires));
        assert_eq!(redraw.schedule(start), (false, Some(expires)));
        let retry = expires + EDITOR_REDRAW_RETRY;
        assert_eq!(redraw.schedule(expires), (true, Some(retry)));

        // Surface acquisition fails: there is no call to presented().
        assert_eq!(redraw.schedule(expires), (false, Some(retry)));
        assert_eq!(redraw.schedule(retry), (true, Some(retry + EDITOR_REDRAW_RETRY)));
        redraw.presented(None);
        assert_eq!(redraw.schedule(retry), (false, None));

        // A clear or an epoch cut also needs to erase the previous pixels.
        redraw.change();
        assert_eq!(redraw.schedule(retry), (true, Some(retry + EDITOR_REDRAW_RETRY)));
        redraw.presented(None);
        assert_eq!(redraw.schedule(retry), (false, None));
    }
}
