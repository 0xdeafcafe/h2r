//! [`Backend`] on `gfx`: surfaces in a `SurfaceStore`, draws through
//! `gfx::Renderer`, textures through `gfx::upload`.
//!
//! What renders today: pre-transformed (`XYZRHW | DIFFUSE`) fixed-function
//! draws via gfx's built-in shader. Everything else needs a
//! [`ShaderProvider`] (TODO(shaders)) and is skipped with a reason until one
//! is installed.
//!
//! gfx changes this would benefit from (not made here; gfx is another lane):
//! * per-draw viewport and scissor in `gfx::Draw` (the renderer always uses
//!   the full target, so the device stops on a non-full viewport or
//!   scissor);
//! * alpha test in the built-in pre-transformed shader (alpha-tested draws
//!   without a shader provider stop);
//! * indexed draws in `gfx::Draw` (the device de-indexes on the CPU);
//! * batching several draws into one pass across calls (the backend opens a
//!   pass per draw, correct but slow);
//! * partial-rectangle clears (`D3DDevice_Clear` with rects).

use gfx::renderer::SHADER_XYZRHW_DIFFUSE;
use gfx::surface::{SurfaceDesc, SurfaceId, SurfaceKind, SurfaceStore};
use gfx::texture::DecodedTexture;
use gfx::{Clear, D3DFormat, Draw, GfxContext, Renderer, VertexStream};

use crate::backend::{Backend, DrawOutcome, ResolvedDraw};
use crate::shader::{ShaderProvider, VertexProcessing};

/// Called on every `Swap` with the surface shown and the swap flags.
pub type PresentHook = Box<dyn FnMut(&GfxContext, &SurfaceStore, SurfaceId, u32) + Send>;

/// Rendering through `gfx`.
pub struct GfxBackend {
    /// The GPU context.
    pub ctx: GfxContext,
    /// All surfaces.
    pub store: SurfaceStore,
    /// The draw path.
    pub renderer: Renderer,
    shaders: Option<Box<dyn ShaderProvider>>,
    presented: Option<SurfaceId>,
    present_hook: Option<PresentHook>,
}

impl GfxBackend {
    /// A backend on `ctx`.
    pub fn new(ctx: GfxContext) -> Self {
        let store = SurfaceStore::new(ctx.caps);
        let renderer = Renderer::new(&ctx);
        Self {
            ctx,
            store,
            renderer,
            shaders: None,
            presented: None,
            present_hook: None,
        }
    }

    /// Installs the shader translator for programmable draws.
    pub fn with_shaders(mut self, shaders: Box<dyn ShaderProvider>) -> Self {
        self.shaders = Some(shaders);
        self
    }

    /// Called on every `Swap` with the surface to show (the platform layer
    /// blits it to its window with `gfx::surface::present_surface`).
    pub fn on_present(
        mut self,
        hook: impl FnMut(&GfxContext, &SurfaceStore, SurfaceId, u32) + Send + 'static,
    ) -> Self {
        self.present_hook = Some(Box::new(hook));
        self
    }

    /// The surface shown by the last `Swap`.
    pub fn last_presented(&self) -> Option<SurfaceId> {
        self.presented
    }

    fn pass(
        &mut self,
        color: SurfaceId,
        depth: Option<SurfaceId>,
        clear: Clear,
        draws: &[Draw<'_>],
    ) -> Result<(), String> {
        let mut enc = self.ctx.device.create_command_encoder(&Default::default());
        if let Some(d) = depth {
            self.store
                .prepare_target(&self.ctx, &mut enc, color, d)
                .map_err(|e| e.to_string())?;
        }
        {
            let target = self.store.target(color, depth).map_err(|e| e.to_string())?;
            self.renderer
                .render(&self.ctx, &mut enc, &target, clear, draws)
                .map_err(|e| e.to_string())?;
        }
        self.store
            .finish_target(&mut enc, color)
            .map_err(|e| e.to_string())?;
        self.ctx.queue.submit([enc.finish()]);
        Ok(())
    }
}

impl Backend for GfxBackend {
    type Surface = SurfaceId;
    type Texture = wgpu::Texture;

    fn create_surface(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
        depth: bool,
    ) -> Result<SurfaceId, String> {
        let kind = if depth {
            SurfaceKind::DepthStencil
        } else {
            SurfaceKind::Color
        };
        self.store
            .create(&self.ctx, SurfaceDesc::new(format, width, height), kind)
            .map_err(|e| e.to_string())
    }

    fn destroy_surface(&mut self, surface: SurfaceId) -> Result<(), String> {
        if self.store.remove(surface) {
            Ok(())
        } else {
            Err(format!("surface {surface:?} is not in the store"))
        }
    }

    fn upload_texture(&mut self, t: &DecodedTexture) -> Result<wgpu::Texture, String> {
        Ok(gfx::upload::upload_texture(
            &self.ctx.device,
            &self.ctx.queue,
            t,
            "guest texture",
        ))
    }

    fn bc_supported(&self) -> bool {
        self.ctx.caps.texture_compression_bc
    }

    fn clear(
        &mut self,
        color: SurfaceId,
        depth: Option<SurfaceId>,
        clear: Clear,
    ) -> Result<(), String> {
        self.pass(color, depth, clear, &[])
    }

    fn draw(
        &mut self,
        color: SurfaceId,
        depth: Option<SurfaceId>,
        d: &ResolvedDraw,
        textures: [Option<&wgpu::Texture>; 4],
    ) -> Result<DrawOutcome, String> {
        let streams: Vec<VertexStream<'_>> = d
            .streams
            .iter()
            .map(|s| VertexStream {
                data: &s.data,
                stride: s.stride,
            })
            .collect();
        let fixed = matches!(d.shader.vertex, VertexProcessing::FixedFunction { .. });
        let builtin_ok = fixed
            && d.layout.pretransformed
            && d.shader.pixel_shader.is_none()
            && textures.iter().all(Option::is_none);
        if builtin_ok {
            if d.state.render.alpha_test_enable {
                // The built-in shader never discards, so alpha-tested pixels
                // would be written: stop rather than render wrongly.
                return Ok(DrawOutcome::Unsupported(
                    "alpha test with the built-in pre-transformed shader (it never discards)"
                        .into(),
                ));
            }
            let draw = Draw {
                shader: SHADER_XYZRHW_DIFFUSE,
                streams: &streams,
                vertex_count: d.vertex_count,
                layout: &d.layout,
                primitive: d.primitive,
                state: d.state.render,
                bind_groups: &[],
            };
            return match self.pass(color, depth, Clear::NONE, &[draw]) {
                Ok(()) => Ok(DrawOutcome::Drawn),
                // The built-in shader rejects layouts without float4 position
                // and D3DCOLOR diffuse: that is a skip, not a failure.
                Err(e) if e.contains("built-in shader") => Ok(DrawOutcome::Skipped(e)),
                Err(e) => Err(e),
            };
        }
        let Some(shaders) = self.shaders.as_mut() else {
            let what = if fixed {
                "fixed-function draw with textures, a pixel shader, or untransformed vertices"
            } else {
                "vertex program"
            };
            return Ok(DrawOutcome::Skipped(format!(
                "{what}: no shader provider (TODO(shaders): wire crates/shaders)"
            )));
        };
        let views: Vec<Option<wgpu::TextureView>> = textures
            .iter()
            .map(|t| t.map(|t| t.create_view(&Default::default())))
            .collect();
        let view_refs = std::array::from_fn(|i| views[i].as_ref());
        let prepared = match shaders.prepare(&self.ctx, &mut self.renderer, d, view_refs) {
            Ok(p) => p,
            Err(e) => return Ok(DrawOutcome::Skipped(e)),
        };
        let groups: Vec<(u32, &wgpu::BindGroup)> =
            prepared.bind_groups.iter().map(|(i, g)| (*i, g)).collect();
        let draw = Draw {
            shader: prepared.id,
            streams: &streams,
            vertex_count: d.vertex_count,
            layout: &d.layout,
            primitive: d.primitive,
            state: d.state.render,
            bind_groups: &groups,
        };
        self.pass(color, depth, Clear::NONE, &[draw])?;
        Ok(DrawOutcome::Drawn)
    }

    fn present(&mut self, surface: SurfaceId, flags: u32) -> Result<(), String> {
        self.presented = Some(surface);
        if let Some(hook) = self.present_hook.as_mut() {
            hook(&self.ctx, &self.store, surface, flags);
        }
        if flags & 4 != 0 {
            // D3DSWAP_FINISH: wait for the GPU.
            let _ = self.ctx.device.poll(wgpu::PollType::wait_indefinitely());
        }
        Ok(())
    }
}
