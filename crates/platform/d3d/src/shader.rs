//! What the device needs from the shader translator.
//!
//! The translation inputs ([`ShaderInputs`], [`Microcode`], the constant
//! blocks) live in `gfx::guest_program`, re-exported here. `crate::nv2a::Nv2aShaders`
//! implements [`ShaderProvider`] over `crates/formats/shaders` (NV2A vertex
//! microcode and `D3DPIXELSHADERDEF` register-combiner programs to WGSL,
//! joined by `shaders::link`); `GfxBackend::new` installs it. A draw it
//! cannot translate is reported as `Event::DrawSkipped` with the reason.

#[cfg(feature = "gpu")]
use gfx::pipeline::ShaderId;
pub use gfx::guest_program::{
    CONSTANTS, Constants, Microcode, PROGRAM_SLOTS, ShaderInputs, VertexProcessing,
    recycle_constants, spare_constants,
};

/// Where a vertex program puts its clip position: the device-side analysis
/// (`crate::clip_space`) cached nowhere, run on demand.
pub trait MicrocodeClip {
    /// Where the program's clip position comes from.
    #[cfg(feature = "gpu")]
    fn clip_position(&self) -> crate::clip_space::epilogue::Position;

    /// The position for diagnostics: `rows(b)`, `far(b)`, `fixed` or
    /// `unknown`. Empty without the `gpu` feature.
    fn clip_text(&self) -> String;
}

impl MicrocodeClip for Microcode {
    #[cfg(feature = "gpu")]
    fn clip_position(&self) -> crate::clip_space::epilogue::Position {
        crate::clip_space::epilogue::position(self)
    }

    fn clip_text(&self) -> String {
        #[cfg(feature = "gpu")]
        {
            use crate::clip_space::epilogue::Position;
            match self.clip_position() {
                Position::Rows(b) => format!("rows({b})"),
                Position::FarPlaneRows(b) => format!("far({b})"),
                Position::Fixed => "fixed".into(),
                Position::Unknown => "unknown".into(),
            }
        }
        #[cfg(not(feature = "gpu"))]
        String::new()
    }
}

#[cfg(feature = "gpu")]
/// A compiled program plus the per-draw resources it reads.
pub struct PreparedShader {
    /// Registered gfx shader id.
    pub id: ShaderId,
    /// Bind groups for `Draw::bind_groups`.
    pub bind_groups: Vec<gfx::BoundGroup>,
    /// The fragment shader can `discard`. With merged passes under the
    /// exact rule such a draw starts a render pass of its own (see
    /// [`GfxBackend::set_merge_masks`](crate::GfxBackend::set_merge_masks)).
    pub discards: bool,
    /// The fragment shader writes the depth (W-buffering), polygon offset
    /// included: the pipeline's own depth bias must stay off.
    pub writes_depth: bool,
}

#[cfg(feature = "gpu")]
/// Turns a draw into a registered gfx shader.
///
/// `Send` because the device lives behind the runtime's `Send + Sync`
/// override handlers.
pub trait ShaderProvider: Send {
    /// Translates (cached by the implementation) and prepares per-draw
    /// uniforms. `draw` carries everything the guest set for this draw:
    /// [`ShaderInputs`] (`draw.shader`), translated and raw render/stage
    /// state (addressing, filtering, alpha kill, bump environment, alpha
    /// reference, combiner registers), and target and texture dimensions
    /// and formats. `textures[n]` is the bound texture view of stage `n`.
    /// An error means "cannot translate"; the draw is then skipped with an
    /// event.
    fn prepare(
        &mut self,
        ctx: &gfx::GfxContext,
        renderer: &mut gfx::Renderer,
        draw: &crate::backend::ResolvedDraw,
        textures: [Option<&wgpu::TextureView>; 4],
    ) -> Result<PreparedShader, String>;

    /// The draws prepared from now on go to a target `scale` times the
    /// guest's size (`crate::scaled`; 1, the default, is the guest's size).
    /// Window coordinates stay the guest's; the provider shifts positions
    /// by [`gfx::renderer::window_shift`] so edges cover what they cover
    /// at 1x.
    fn set_render_scale(&mut self, _scale: crate::scaled::RenderScale) {}

    /// E3 lighting's hard shadows (docs/graphics/e3-lighting-setting.md):
    /// from now on every shadow-map comparison is one tap
    /// (`shaders::psh::PixelOptions::hard_shadow`). Only a render-scale
    /// view is told this; off (the default) is retail's lookup.
    fn set_hard_shadows(&mut self, _on: bool) {}

    /// E3 lighting's Sharp edge on the native frame graph
    /// (docs/graphics/e3-lighting.md, "The edge"): while on, every shadow
    /// stage of the draws prepared uses `PixelOptions::sharp_shadow`, which
    /// takes precedence over [`set_hard_shadows`](Self::set_hard_shadows).
    /// The graph turns it on for each receiver of a shadow tile only.
    fn set_sharp_shadows(&mut self, _on: bool) {}

    /// Writes per-draw data staged by [`prepare`](Self::prepare) to the
    /// queue. The backend calls it before every submit.
    fn flush_uploads(&mut self, _ctx: &gfx::GfxContext) {}

    /// Every draw prepared so far has been submitted: staging space may be
    /// reused.
    fn submitted(&mut self) {}

    /// A frame was presented (per-frame caches may drop what went unused).
    fn end_frame(&mut self) {}

    /// `gone` were destroyed (frame-buffer copies the backend released):
    /// drop cached bind groups over them.
    fn forget_textures(&mut self, _gone: &[wgpu::Texture]) {}

    /// Called once when the backend is set up and then after every frame,
    /// with the pipelines draws needed since the last call that were not
    /// built yet ([`gfx::Renderer::take_new_keys`]). A provider with a
    /// pipeline cache records them and prewarms from it here.
    fn maintain(
        &mut self,
        _ctx: &gfx::GfxContext,
        _renderer: &mut gfx::Renderer,
        _new_pipelines: &[(gfx::pipeline::PipelineKey, bool)],
    ) {
    }

    /// A [`ShaderJudge`] that answers, without a GPU, whether
    /// [`prepare`](Self::prepare) succeeds for a draw. `None` (the
    /// default): it cannot be predicted.
    fn judge(&self, caps: gfx::translate::Capabilities) -> Option<Box<dyn ShaderJudge>> {
        let _ = caps;
        None
    }
}

#[cfg(feature = "gpu")]
/// Predicts [`ShaderProvider::prepare`]'s verdict on the device's side, so a
/// render thread can answer a draw's outcome when it is issued
/// (`crate::threaded`).
pub trait ShaderJudge: Send {
    /// `Err(reason)` exactly where `prepare` fails. `present[n]`: stage
    /// `n` has a sampleable texture view.
    fn check(&mut self, draw: &crate::backend::ResolvedDraw, present: [bool; 4]) -> Result<(), String>;
}
