//! [`Backend`] on `gfx`: surfaces in a `SurfaceStore`, draws through
//! `gfx::Renderer`, textures through `gfx::upload`.
//!
//! What renders:
//! * pre-transformed (`XYZRHW | DIFFUSE`) fixed-function draws whose whole
//!   state the built-in shader reproduces (see
//!   [`builtin_equivalent`](crate::nv2a::builtin_equivalent): no texture,
//!   fog or specular, stages passing diffuse through) through gfx's built-in
//!   shader (alpha test included), with the depth format's z full scale;
//! * everything else through the installed [`ShaderProvider`]:
//!   [`Nv2aShaders`] by default (translated NV2A vertex programs and
//!   register combiners, plus the fixed-function texture stages turned into
//!   combiners). A draw the provider cannot translate is skipped with the
//!   reason.
//!
//! Every draw is clipped to the viewport rectangle with a scissor: vertex
//! positions are already window coordinates (pre-transformed vertices, and
//! NV2A programs apply the viewport themselves), so the wgpu viewport stays
//! the whole target.
//!
//! Not used yet (the device does not hand these over): gfx's partial and
//! channel-masked clears (`gfx::Renderer::clear_rects`), indexed draws
//! (`gfx::Draw::indices`; the device de-indexes on the CPU), scissor
//! rectangles beyond the viewport, and render-to-texture surfaces
//! (`gfx::surface::SurfaceStore::create_texture_target`). The backend
//! opens one pass per draw, correct but slow.

use gfx::renderer::{SHADER_XYZRHW_DIFFUSE, d3dz_max};
use gfx::surface::{SurfaceDesc, SurfaceId, SurfaceKind, SurfaceStore};
use gfx::texture::DecodedTexture;
use gfx::texture::TextureKind;
use gfx::{Clear, D3DFormat, Draw, GfxContext, Renderer, ScissorRect, VertexStream};

use crate::backend::{Backend, DrawOutcome, ResolvedDraw};
use crate::nv2a::{Nv2aShaders, builtin_equivalent};
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
    /// A backend on `ctx`, translating shaders with [`Nv2aShaders`].
    pub fn new(ctx: GfxContext) -> Self {
        let store = SurfaceStore::new(ctx.caps);
        let renderer = Renderer::new(&ctx);
        Self {
            ctx,
            store,
            renderer,
            shaders: Some(Box::new(Nv2aShaders::new())),
            presented: None,
            present_hook: None,
        }
    }

    /// Replaces the shader translator for textured and programmable draws.
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

    /// Removes the shader translator: only draws the built-in shader handles
    /// render; the rest are skipped.
    pub fn without_shaders(mut self) -> Self {
        self.shaders = None;
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
        // Window-coordinate geometry: the viewport only clips.
        let v = d.viewport;
        let scissor = Some(ScissorRect::from_edges(
            v.x as i64,
            v.y as i64,
            v.x as i64 + v.width as i64,
            v.y as i64 + v.height as i64,
        ));
        let fixed = matches!(d.shader.vertex, VertexProcessing::FixedFunction { .. });
        let textured = textures.iter().any(Option::is_some);
        let readable = fixed && Renderer::builtin_supports(&d.layout);
        // With a provider the built-in shader only takes draws it renders
        // exactly; without one it stands in for every untextured
        // fixed-function draw it can read (stage states, fog and specular
        // ignored).
        let builtin_ok = readable
            && if self.shaders.is_some() {
                builtin_equivalent(d, textured)
            } else {
                !textured && d.shader.pixel_shader.is_none()
            };
        if builtin_ok || self.shaders.is_none() {
            if !builtin_ok {
                let what = if fixed {
                    "fixed-function draw with textures, a pixel shader, or vertices the built-in shader cannot read"
                } else {
                    "vertex program"
                };
                return Ok(DrawOutcome::Skipped(format!("{what}: no shader provider")));
            }
            self.renderer
                .set_pretransformed_z_max(d3dz_max(d.target.depth_format));
            let draw = Draw {
                scissor,
                ..Draw::new(
                    SHADER_XYZRHW_DIFFUSE,
                    &d.layout,
                    &streams,
                    d.vertex_count,
                    d.primitive,
                    d.state.render,
                )
            };
            self.pass(color, depth, Clear::NONE, &[draw])?;
            return Ok(DrawOutcome::Drawn);
        }
        let shaders = self.shaders.as_mut().expect("checked above");
        // Views as the shaders sample them; a texture that cannot be bound
        // as a filterable float texture is left out (the program reads a
        // stand-in).
        let features = self.ctx.device.features();
        let views: Vec<Option<wgpu::TextureView>> = textures
            .iter()
            .zip(&d.texture_info)
            .map(|(t, info)| {
                let (t, info) = (t.as_ref()?, info.as_ref()?);
                if !gfx::program::filterable(t.format(), features) {
                    return None;
                }
                let dimension = match info.kind {
                    TextureKind::D2 => wgpu::TextureViewDimension::D2,
                    TextureKind::Cube => wgpu::TextureViewDimension::Cube,
                    TextureKind::Volume => wgpu::TextureViewDimension::D3,
                };
                Some(t.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(dimension),
                    ..Default::default()
                }))
            })
            .collect();
        let view_refs = std::array::from_fn(|i| views[i].as_ref());
        let prepared = match shaders.prepare(&self.ctx, &mut self.renderer, d, view_refs) {
            Ok(p) => p,
            Err(e) => return Ok(DrawOutcome::Skipped(e)),
        };
        let groups: Vec<(u32, &wgpu::BindGroup)> =
            prepared.bind_groups.iter().map(|(i, g)| (*i, g)).collect();
        let draw = Draw {
            bind_groups: &groups,
            scissor,
            ..Draw::new(
                prepared.id,
                &d.layout,
                &streams,
                d.vertex_count,
                d.primitive,
                d.state.render,
            )
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
