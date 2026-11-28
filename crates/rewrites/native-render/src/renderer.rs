//! The renderer (docs/graphics/native-renderer.md, "How a frame flows"):
//! it owns the GPU device and queue, the display's targets at the render
//! scale and the upload arenas, and draws each [`Frame`] through the frame
//! graph. Hosts and `h2 sim` build none.

use frame_scene::FrameScene;
use frame_types::TargetRef;

use crate::frame_graph::{self, Cameras, CapturedFrame, FrameReport, Gpu, Targets};
use crate::gpu::Image;
use crate::lighting::Lighting;
use crate::render_scale::RenderScale;
use crate::stage::Frame;

pub struct Renderer {
    ctx: gfx::GfxContext,
    /// The display's size in pixels.
    output: (u32, u32),
    targets: Targets,
    lighting: Lighting,
    uploads: gfx::ring::UploadPool,
    blitter: gfx::blit::Blitter,
}

fn scale_for(output: (u32, u32)) -> RenderScale {
    RenderScale::for_output(output.0, output.1).unwrap_or_else(|e| panic!("renderer: output {output:?}: {e}"))
}

impl Renderer {
    /// A renderer on `ctx` for a display of `output` pixels, with the
    /// launch's lighting (`H2_LIGHTING`).
    pub fn new(ctx: gfx::GfxContext, output: (u32, u32)) -> Self {
        let lighting = crate::lighting::lighting().unwrap_or_else(|e| panic!("renderer: {e}"));
        let blitter = gfx::blit::Blitter::new(&ctx);
        Self {
            ctx,
            output,
            targets: Targets::new(scale_for(output)),
            lighting,
            uploads: gfx::ring::UploadPool::default(),
            blitter,
        }
    }

    pub fn context(&self) -> &gfx::GfxContext {
        &self.ctx
    }

    pub fn output(&self) -> (u32, u32) {
        self.output
    }

    pub fn lighting(&self) -> Lighting {
        self.lighting
    }

    /// The display's new size: its targets are made again at the scale
    /// that fills it.
    pub fn resize(&mut self, output: (u32, u32)) {
        self.output = output;
        self.targets.set_scale(scale_for(output));
    }

    /// `f` drawn on the display's targets, with each view's camera blended
    /// from `from`'s scene by its fraction when given. Returns the cameras
    /// it drew with.
    pub fn draw(&mut self, f: &mut Frame, from: Option<(&FrameScene, f32)>) -> (FrameReport, Cameras) {
        let cameras = Cameras::of(&f.scene, from);
        let gpu = Gpu { device: &self.ctx.device, queue: &self.ctx.queue, uploads: &self.uploads };
        let report = frame_graph::walk(&gpu, &mut self.targets, f, &cameras, self.lighting);
        (report, cameras)
    }

    /// Shows the last drawn frame on `presenter`'s surface. `false` when
    /// the surface had no frame to give (occluded, or just reconfigured).
    pub fn present(&mut self, presenter: &mut gfx::Presenter) -> bool {
        let acquired = presenter.acquire(&self.ctx).unwrap_or_else(|e| panic!("renderer: present: {e}"));
        let gfx::Acquired::Frame(frame) = acquired else {
            return false;
        };
        let colour = self.targets.get(&self.ctx.device, TargetRef::SceneColour).expect("scene colour");
        let mut encoder = self.ctx.device.create_command_encoder(&Default::default());
        {
            let target = presenter.target(&frame);
            let opts = gfx::blit::BlitOptions { linear: true, force_opaque: true, ..Default::default() };
            self.blitter.blit(&self.ctx, &mut encoder, &colour.view, target.color, target.color_format, opts);
        }
        self.ctx.queue.submit([encoder.finish()]);
        presenter.present(&self.ctx, frame);
        true
    }

    /// `f` drawn privately at `size` and read back (screenshots, film
    /// thumbnails, `render.capture`).
    pub fn capture(&mut self, f: &mut Frame, size: (u32, u32)) -> Image {
        let captured = self.capture_frame(f, size);
        Image::from_rgba(captured.size[0], captured.size[1], captured.rgba, Vec::new())
    }

    /// [`Self::capture`] with the scene it came from and what it drew.
    pub fn capture_frame(&mut self, f: &mut Frame, size: (u32, u32)) -> CapturedFrame {
        let gpu = Gpu { device: &self.ctx.device, queue: &self.ctx.queue, uploads: &self.uploads };
        frame_graph::capture(&gpu, &self.ctx, f, scale_for(size), self.lighting)
    }
}
