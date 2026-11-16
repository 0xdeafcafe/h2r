//! Opens a window and draws a test triangle plus a translucent quad through
//! the D3D8-style state model.
//!
//! This file is the *platform layer*: everything winit-specific lives here.
//! The `gfx` crate only ever sees a `wgpu::Surface`, so a tvOS or Xbox shell
//! can replace this file without touching the core.
//!
//! ```text
//! cargo run -p gfx --example window            # runs until closed
//! cargo run -p gfx --example window -- --frames 120
//! ```
//!
//! Keys: `C` cycles the cull mode, `W` toggles wireframe, `B` toggles
//! alpha blending of the quad. State is set exactly the way the HLE layer
//! will set it: raw Xbox values into [`RenderState::set_raw`].

use std::sync::Arc;

use gfx::repack::FetchMode;
use gfx::state::{RenderStateType, RenderState};
use gfx::vertex::{PrimitiveType, VertexLayout, fvf};
use gfx::renderer::SHADER_XYZRHW_DIFFUSE;
use gfx::{Acquired, Clear, Draw, GfxContext, Presenter, Renderer, VertexStream};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

// Xbox render-state slot indices (XDK 5933 numbering) and values.
const RS_ALPHABLENDENABLE: u32 = 59;
const RS_SRCBLEND: u32 = 62;
const RS_DESTBLEND: u32 = 63;
const RS_FILLMODE: u32 = 139;
const RS_CULLMODE: u32 = 147;
const BLEND_SRCALPHA: u32 = 0x302;
const BLEND_INVSRCALPHA: u32 = 0x303;
const FILL_WIREFRAME: u32 = 0x1B01;
const FILL_SOLID: u32 = 0x1B02;
const CULL_NONE: u32 = 0;
const CULL_CW: u32 = 0x900;
const CULL_CCW: u32 = 0x901;

/// Packs one `XYZRHW | DIFFUSE` vertex: x, y (pixels), z, rhw, 0xAARRGGBB.
fn vertex(x: f32, y: f32, color: u32) -> [u8; 20] {
    let mut v = [0u8; 20];
    for (i, f) in [x, y, 0.5 * 16_777_215.0, 1.0].iter().enumerate() {
        v[i * 4..i * 4 + 4].copy_from_slice(&f.to_le_bytes());
    }
    v[16..20].copy_from_slice(&color.to_le_bytes());
    v
}

struct Gpu {
    window: Arc<Window>,
    ctx: GfxContext,
    presenter: Presenter,
    renderer: Renderer,
}

struct App {
    gpu: Option<Gpu>,
    layout: VertexLayout,
    cull: u32,
    wireframe: bool,
    blend: bool,
    frames_left: Option<u32>,
}

impl App {
    fn render_state(&self, blend: bool) -> RenderState {
        let mut rs = RenderState::default();
        let mut set = |idx: u32, v: u32| rs.set_raw(RenderStateType::from_index(idx), v).expect("valid Xbox state value");
        set(RS_CULLMODE, self.cull);
        set(RS_FILLMODE, if self.wireframe { FILL_WIREFRAME } else { FILL_SOLID });
        if blend {
            set(RS_ALPHABLENDENABLE, 1);
            set(RS_SRCBLEND, BLEND_SRCALPHA);
            set(RS_DESTBLEND, BLEND_INVSRCALPHA);
        }
        rs
    }

    fn redraw(&mut self) {
        let (opaque, translucent) = (self.render_state(false), {
            let mut s = self.render_state(self.blend);
            s.cull_mode = gfx::state::CullMode::None;
            s
        });
        let Some(gpu) = self.gpu.as_mut() else { return };
        let (w, h) = gpu.presenter.size();
        let (w, h) = (w as f32, h as f32);
        // Triangle, wound clockwise on screen (y down): top, bottom-right, bottom-left.
        let tri: Vec<u8> = [
            vertex(w * 0.5, h * 0.15, 0xFFFF0000),
            vertex(w * 0.85, h * 0.8, 0xFF00FF00),
            vertex(w * 0.15, h * 0.8, 0xFF0000FF),
        ]
        .concat();
        // A quad list (Xbox-only primitive) drawn semi-transparent over it.
        let quad: Vec<u8> = [
            vertex(w * 0.3, h * 0.4, 0x80FFFFFF),
            vertex(w * 0.7, h * 0.4, 0x80FFFF00),
            vertex(w * 0.7, h * 0.65, 0x80FF00FF),
            vertex(w * 0.3, h * 0.65, 0x80FFFFFF),
        ]
        .concat();

        let draws = [
            Draw {
                shader: SHADER_XYZRHW_DIFFUSE,
                streams: &[VertexStream { data: &tri, stride: 0 }],
                bind_groups: &[],
                viewport: None,
                scissor: None,
                indices: None,
                vertex_count: 3,
                layout: &self.layout,
                primitive: PrimitiveType::TriangleList,
                state: opaque,
                count_samples: false,
                window: None,
                vertex_fetch: FetchMode::Cpu,
                stream_ids: &[],
            },
            Draw {
                shader: SHADER_XYZRHW_DIFFUSE,
                streams: &[VertexStream { data: &quad, stride: 0 }],
                bind_groups: &[],
                viewport: None,
                scissor: None,
                indices: None,
                vertex_count: 4,
                layout: &self.layout,
                primitive: PrimitiveType::QuadList,
                state: translucent,
                count_samples: false,
                window: None,
                vertex_fetch: FetchMode::Cpu,
                stream_ids: &[],
            },
        ];

        match gpu.presenter.acquire(&gpu.ctx) {
            Ok(Acquired::Frame(frame)) => {
                let mut enc = gpu.ctx.device.create_command_encoder(&Default::default());
                let target = gpu.presenter.target(&frame);
                if let Err(e) = gpu.renderer.render(&gpu.ctx, &mut enc, &target, Clear::all([0.05, 0.05, 0.12, 1.0]), &draws) {
                    eprintln!("draw failed: {e}");
                }
                gpu.ctx.queue.submit([enc.finish()]);
                gpu.window.pre_present_notify();
                gpu.presenter.present(&gpu.ctx, frame);
            }
            Ok(Acquired::Skip) => {}
            Err(e) => eprintln!("frame error: {e}"),
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        let window = Arc::new(
            event_loop
                .create_window(Window::default_attributes().with_title("h2recomp gfx: test triangle"))
                .expect("create window"),
        );
        let size = window.inner_size();
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone()).expect("create surface");
        let ctx = pollster::block_on(GfxContext::new(instance, Some(&surface))).expect("init gpu");
        let presenter = Presenter::new(&ctx, surface, (size.width, size.height)).expect("configure surface");
        let renderer = Renderer::new(&ctx);
        println!("adapter: {:?}, caps: {:?}", ctx.adapter.get_info().name, ctx.caps);
        window.request_redraw();
        self.gpu = Some(Gpu { window, ctx, presenter, renderer });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(g) = self.gpu.as_mut() {
                    g.presenter.resize(&g.ctx, (size.width, size.height));
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed && !event.repeat => {
                match event.logical_key {
                    Key::Character(ref c) if c.eq_ignore_ascii_case("c") => {
                        self.cull = match self.cull {
                            CULL_NONE => CULL_CW,
                            CULL_CW => CULL_CCW,
                            _ => CULL_NONE,
                        };
                        println!("cull mode: 0x{:x}", self.cull);
                    }
                    Key::Character(ref c) if c.eq_ignore_ascii_case("w") => self.wireframe = !self.wireframe,
                    Key::Character(ref c) if c.eq_ignore_ascii_case("b") => self.blend = !self.blend,
                    Key::Named(NamedKey::Escape) => event_loop.exit(),
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => {
                self.redraw();
                if let Some(n) = self.frames_left.as_mut() {
                    *n = n.saturating_sub(1);
                    if *n == 0 {
                        println!("rendered requested frames; exiting");
                        event_loop.exit();
                        return;
                    }
                }
                if let Some(g) = &self.gpu {
                    g.window.request_redraw();
                }
            }
            _ => {}
        }
    }
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut frames_left = None;
    while let Some(a) = args.next() {
        if a == "--frames" {
            frames_left = Some(args.next().and_then(|v| v.parse().ok()).ok_or_else(|| anyhow::anyhow!("--frames needs a number"))?);
        }
    }
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        gpu: None,
        layout: VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE)?,
        cull: CULL_CCW,
        wireframe: false,
        blend: true,
        frames_left,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}
