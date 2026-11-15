//! What the device needs from the shader translator.
//!
//! [`crate::nv2a::Nv2aShaders`] implements [`ShaderProvider`] over
//! `crates/shaders` (NV2A vertex microcode and `D3DPIXELSHADERDEF`
//! register-combiner programs to WGSL, joined by `shaders::link`);
//! `GfxBackend::new` installs it. A draw it cannot translate is reported as
//! `Event::DrawSkipped` with the reason.
//!
//! The device side is complete: it captures everything a translated draw
//! needs into [`ShaderInputs`] at draw time, from the device model (program
//! memory, constants) and the guest arrays (combiner state is render state).

use gfx::pipeline::ShaderId;

/// NV2A vertex program slots.
pub const PROGRAM_SLOTS: usize = 136;
/// Vertex shader constant registers (`c[-96..95]`, stored 0..191).
pub const CONSTANTS: usize = 192;

/// How vertices are processed for a draw.
#[derive(Clone, Debug, PartialEq)]
pub enum VertexProcessing {
    /// Fixed-function transform selected by an FVF code (`SetVertexShader`
    /// with an even handle).
    FixedFunction {
        /// The FVF.
        fvf: u32,
    },
    /// A vertex program in program memory.
    Program {
        /// First slot (`SelectVertexShaderDirect`/`LoadVertexShader` address).
        start: u32,
        /// Microcode from `start` up to and including the final instruction
        /// (4 dwords per slot).
        microcode: Vec<u32>,
    },
}

/// Everything a translated shader pair consumes for one draw.
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderInputs {
    /// Vertex side.
    pub vertex: VertexProcessing,
    /// Vertex registers the layout supplies (bit n = `v`n).
    pub provided_inputs: u16,
    /// `SetVertexData*` defaults for registers the layout does not supply.
    pub vertex_defaults: [[f32; 4]; 16],
    /// Constant registers, hardware order (index = register + 96).
    pub constants: Box<[[f32; 4]; CONSTANTS]>,
    /// The 60-dword `D3DPIXELSHADERDEF`, or `None` for the fixed-function
    /// combiner D3D derives from the texture-stage states.
    pub pixel_shader: Option<Box<[u32; 60]>>,
    /// Render-state slots 0..57 (`PS*`: combiner inputs/outputs/constants as
    /// the guest last wrote them; constants can change without a new PSDEF).
    pub combiner_state: Box<[u32; 57]>,
    /// Device `+0x788` at the draw: whether the definition bound by
    /// `SetPixelShader` had a final combiner (`PSDEF[8] | PSDEF[9]`, latched
    /// at bind). The lazy commit reads this, not the definition, so it
    /// survives later zero pushes and reuse of the definition's buffer.
    pub final_combiner_latch: bool,
    /// Device flag 2 (`+0x8`) at the draw: the projection is affine. With a
    /// fog table, pre-transformed vertices then take their fog coordinate
    /// from z instead of w (`0x3f6f40`; `nv2a::PassthroughFog`).
    pub affine_projection: bool,
}

/// A compiled program plus the per-draw resources it reads.
pub struct PreparedShader {
    /// Registered gfx shader id.
    pub id: ShaderId,
    /// Bind groups for `Draw::bind_groups`.
    pub bind_groups: Vec<gfx::BoundGroup>,
    /// The fragment shader can `discard`. With merged passes such a draw
    /// starts a render pass of its own after a partly masked draw, or
    /// always under the exact rule (see
    /// [`GfxBackend::set_merge_discards`](crate::GfxBackend::set_merge_discards)).
    pub discards: bool,
    /// The fragment shader writes the depth (W-buffering), polygon offset
    /// included: the pipeline's own depth bias must stay off.
    pub writes_depth: bool,
}

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

    /// Writes per-draw data staged by [`prepare`](Self::prepare) to the
    /// queue. The backend calls it before every submit.
    fn flush_uploads(&mut self, _ctx: &gfx::GfxContext) {}

    /// Every draw prepared so far has been submitted: staging space may be
    /// reused.
    fn submitted(&mut self) {}

    /// A frame was presented (per-frame caches may drop what went unused).
    fn end_frame(&mut self) {}

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

/// Predicts [`ShaderProvider::prepare`]'s verdict on the device's side, so a
/// render thread can answer a draw's outcome when it is issued
/// (`crate::threaded`).
pub trait ShaderJudge: Send {
    /// `Err(reason)` exactly where `prepare` fails. `present[n]`: stage
    /// `n` has a sampleable texture view.
    fn check(&mut self, draw: &crate::backend::ResolvedDraw, present: [bool; 4]) -> Result<(), String>;
}
