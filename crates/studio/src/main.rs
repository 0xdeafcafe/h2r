//! h2-studio: the Dear ImGui editor suite. See crates/studio/README.md.
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use dear_imgui_rs::Context;
use dear_imgui_wgpu::{FramebufferExtent, GammaMode, WgpuInitInfo, WgpuRenderer};
use dear_imgui_winit::{HiDpiMode, WinitPlatform};
use studio::app::{Action, App};
use studio::ui::Studio;
use studio::viewport::Gpu;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

const USAGE: &str = "usage: h2-studio [--disc <iso> | --map <file.map> | --folder <maps dir>] \
[--data <dir>] [--screenshot <out.png> [--size <w>x<h>] [--scale <s>] [--pick <map>]]";

struct Args {
    source: Option<PathBuf>,
    data: Option<PathBuf>,
    screenshot: Option<PathBuf>,
    size: [f32; 2],
    scale: f32,
    pick: Option<String>,
}

fn args() -> Result<Args> {
    let mut a = Args {
        source: None,
        data: None,
        screenshot: None,
        size: [1600.0, 1000.0],
        scale: 2.0,
        pick: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or_else(|| anyhow!("{flag} needs a value\n{USAGE}"));
        match flag.as_str() {
            "--disc" | "--map" | "--folder" => a.source = Some(value()?.into()),
            "--data" => a.data = Some(value()?.into()),
            "--screenshot" => a.screenshot = Some(value()?.into()),
            "--pick" => a.pick = Some(value()?),
            "--scale" => a.scale = value()?.parse()?,
            "--size" => {
                let v = value()?;
                let (w, h) = v
                    .split_once('x')
                    .ok_or_else(|| anyhow!("--size wants <w>x<h>"))?;
                a.size = [w.parse()?, h.parse()?];
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            _ => bail!("unknown argument {flag}\n{USAGE}"),
        }
    }
    Ok(a)
}

/// Dear ImGui's errors as anyhow errors (they need not be `Send`).
fn imgui_err(e: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("{e}")
}

fn ini_path() -> PathBuf {
    content::store::default_data_root().join("h2-studio.ini")
}

fn main() -> Result<()> {
    let args = args()?;
    let mut app = App::new();
    if let Some(d) = &args.data {
        app.set_data_root(d.clone());
    }
    if let Some(src) = editor_core::default_disc(args.source.clone()) {
        app.dispatch(Action::OpenSource(src));
    }
    if let Some(name) = &args.pick {
        app.dispatch(Action::SelectMap(name.clone()));
    }
    if let Some(out) = &args.screenshot {
        let mut h = studio::headless::Headless::new(args.size, args.scale)?;
        h.screenshot(&mut app, 6, out)?;
        println!("wrote {}", out.display());
        return Ok(());
    }
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut runner = Runner { app, win: None };
    event_loop.run_app(&mut runner)?;
    Ok(())
}

struct Runner {
    app: App,
    win: Option<Win>,
}

struct Win {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    platform: WinitPlatform,
    renderer: WgpuRenderer,
    studio: Studio,
    title: String,
    // Declared last: dropped after the platform and renderer.
    ctx: Context,
}

impl Win {
    fn new(event_loop: &ActiveEventLoop, title: String) -> Result<Self> {
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title(&title)
                    .with_inner_size(LogicalSize::new(1600.0, 1000.0)),
            )?,
        );
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let surface = instance.create_surface(window.clone())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))?;
        let caps = surface.get_capabilities(&adapter);
        let format = [
            wgpu::TextureFormat::Bgra8UnormSrgb,
            wgpu::TextureFormat::Rgba8UnormSrgb,
        ]
        .into_iter()
        .find(|f| caps.formats.contains(f))
        .unwrap_or(caps.formats[0]);
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let (mut ctx, fonts) = studio::context(Some(ini_path()));
        let mut platform = WinitPlatform::new(&mut ctx).map_err(imgui_err)?;
        platform
            .attach_window(window.clone(), HiDpiMode::Default, &mut ctx)
            .map_err(imgui_err)?;
        let mut renderer = WgpuRenderer::new(
            WgpuInitInfo::new(device.clone(), queue.clone(), format),
            &mut ctx,
        )
        .map_err(imgui_err)?;
        renderer.set_gamma_mode(GammaMode::Auto);
        Ok(Self {
            window,
            surface,
            config,
            device,
            queue,
            platform,
            renderer,
            studio: Studio::new(fonts),
            title,
            ctx,
        })
    }

    fn resize(&mut self, w: u32, h: u32) {
        if w > 0 && h > 0 {
            self.config.width = w;
            self.config.height = h;
            self.surface.configure(&self.device, &self.config);
        }
    }

    fn redraw(&mut self, app: &mut App) -> Result<()> {
        let (frame, reconfigure) = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => (f, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(f) => (f, true),
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                bail!("surface acquisition failed with a wgpu validation error")
            }
        };
        self.platform
            .prepare_frame(&mut self.ctx, &self.window)
            .map_err(imgui_err)?;
        let ui = self.ctx.frame();
        let mut gpu = Gpu {
            device: &self.device,
            queue: &self.queue,
            renderer: &mut self.renderer,
        };
        self.studio.frame(ui, app, &mut gpu);
        self.platform
            .prepare_render(ui, &self.window)
            .map_err(imgui_err)?;

        let view = frame.texture.create_view(&Default::default());
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("h2-studio"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let consumer = self.renderer.renderer_consumer().map_err(imgui_err)?;
            let pending = self.ctx.try_render(consumer).map_err(imgui_err)?;
            self.renderer
                .render(
                    pending,
                    &mut pass,
                    FramebufferExtent::from_texture(&frame.texture),
                )
                .map_err(imgui_err)?;
        }
        self.queue.submit([enc.finish()]);
        self.queue.present(frame);
        if reconfigure {
            self.surface.configure(&self.device, &self.config);
        }
        let title = app.title();
        if title != self.title {
            self.window.set_title(&title);
            self.title = title;
        }
        Ok(())
    }

    fn shutdown(&mut self) {
        let _ = self.ctx.save_ini_settings_to_disk(ini_path());
        let _ = self.ctx.end_frame();
        if let Err(e) = self.renderer.shutdown(&mut self.ctx) {
            eprintln!("renderer shutdown: {e}");
        }
        if let Err(e) = self.platform.shutdown(&mut self.ctx) {
            eprintln!("platform shutdown: {e}");
        }
    }
}

impl ApplicationHandler for Runner {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.win.is_none() {
            match Win::new(event_loop, self.app.title()) {
                Ok(w) => self.win = Some(w),
                Err(e) => {
                    eprintln!("h2-studio: {e:#}");
                    event_loop.exit();
                }
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(win) = self.win.as_mut() else {
            return;
        };
        if let Err(e) = win.platform.handle_window_event(&mut win.ctx, id, &event) {
            eprintln!("h2-studio: {e}");
            event_loop.exit();
            return;
        }
        match event {
            WindowEvent::Resized(size) => win.resize(size.width, size.height),
            WindowEvent::ScaleFactorChanged { .. } => {
                let size = win.window.inner_size();
                win.resize(size.width, size.height);
            }
            WindowEvent::CloseRequested => self.app.dispatch(Action::Quit),
            WindowEvent::DroppedFile(path) => self.app.dispatch(Action::OpenSource(path)),
            WindowEvent::RedrawRequested => {
                if let Err(e) = win.redraw(&mut self.app) {
                    eprintln!("h2-studio: {e:#}");
                }
            }
            _ => {}
        }
        if self.app.should_quit() {
            win.shutdown();
            event_loop.exit();
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(w) = &self.win {
            w.window.request_redraw();
        }
    }
}
