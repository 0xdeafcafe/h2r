//! The renderer (docs/graphics/native-renderer.md, "How a frame flows"):
//! it owns the GPU device and queue and draws each [`Frame`] through the
//! frame graph. Hosts and `h2 sim` build none.

use crate::gpu::ImagePixels;
use crate::stage::Frame;

pub struct Renderer {
    ctx: gfx::GfxContext,
    /// The display's size in pixels.
    output: (u32, u32),
}

impl Renderer {
    pub fn new(ctx: gfx::GfxContext, output: (u32, u32)) -> Self {
        Self { ctx, output }
    }

    pub fn context(&self) -> &gfx::GfxContext {
        &self.ctx
    }

    pub fn output(&self) -> (u32, u32) {
        self.output
    }

    /// `f` drawn privately at `size` and read back (screenshots, film
    /// thumbnails, `render.capture`).
    pub fn capture(&mut self, f: &Frame, size: (u32, u32)) -> ImagePixels {
        let _ = (f, size);
        unimplemented!("renderer: capture before the frame graph walks Frame.ops")
    }
}
