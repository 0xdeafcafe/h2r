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

/// A draw's constant registers ([`ShaderInputs::constants`]), shared by
/// the draws that see the same registers.
pub type Constants = std::sync::Arc<[[f32; 4]; CONSTANTS]>;

/// Constant blocks of draws already run, for the next draws to fill: with
/// the render thread, a draw's 3 KiB block was allocated on the game
/// thread and freed on the render thread, and the allocator returned the
/// freed pages to the system (`madvise`) as fast.
static SPARE_CONSTANTS: std::sync::Mutex<Vec<Constants>> = std::sync::Mutex::new(Vec::new());

/// Most blocks kept spare (6 MiB).
const SPARE_CONSTANT_BLOCKS: usize = 2048;

/// A constant block no one else holds, to overwrite: a spare one, or a
/// new one. Spares still shared are let go (their holders free them).
pub fn spare_constants() -> Constants {
    if let Ok(mut s) = SPARE_CONSTANTS.lock() {
        while let Some(mut c) = s.pop() {
            if std::sync::Arc::get_mut(&mut c).is_some() {
                return c;
            }
        }
    }
    std::sync::Arc::new([[0.0; 4]; CONSTANTS])
}

/// Keeps the blocks of draws already run for [`spare_constants`], up to a
/// limit, taking the lock once. Blocks other draws or the device still
/// hold are only let go.
pub fn recycle_constants(blocks: &mut Vec<Constants>) {
    blocks.retain(|c| std::sync::Arc::strong_count(c) == 1);
    if !blocks.is_empty()
        && let Ok(mut s) = SPARE_CONSTANTS.lock()
    {
        let room = SPARE_CONSTANT_BLOCKS.saturating_sub(s.len());
        s.extend(blocks.drain(..).take(room));
    }
    blocks.clear();
}

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
        microcode: Microcode,
    },
}

/// Vertex microcode, shared: the device hands every draw of an unchanged
/// program the same words, and the shader keys built from a draw hold them
/// too. A clone is a reference count; equality is pointer equality first
/// (then the words); the hash is the words' hash, made once. It
/// dereferences to the words, and prints and serializes as them (as the
/// `Vec<u32>` it replaces did), so persisted program ids are unchanged.
#[derive(Clone)]
pub struct Microcode {
    words: std::sync::Arc<[u32]>,
    hash: u64,
}

impl Microcode {
    /// Shares `words`, hashing them once.
    pub fn new(words: std::sync::Arc<[u32]>) -> Self {
        let hash = Self::hash_words(&words);
        Self { words, hash }
    }

    /// The hash [`new`](Self::new) gives `words`: FNV-1a over the dwords
    /// (only in-process maps use it).
    pub fn hash_words(words: &[u32]) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for w in words {
            hash = (hash ^ *w as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    /// The words' hash, as [`hash_words`](Self::hash_words) gives it.
    pub fn words_hash(&self) -> u64 {
        self.hash
    }

    /// Whether `self` and `other` share their words (not only equal ones).
    pub fn same(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.words, &other.words)
    }
}

impl From<Vec<u32>> for Microcode {
    fn from(words: Vec<u32>) -> Self {
        Self::new(words.into())
    }
}

impl std::ops::Deref for Microcode {
    type Target = [u32];
    fn deref(&self) -> &[u32] {
        &self.words
    }
}

impl PartialEq for Microcode {
    fn eq(&self, other: &Self) -> bool {
        self.same(other) || (self.hash == other.hash && self.words == other.words)
    }
}

impl Eq for Microcode {}

impl std::hash::Hash for Microcode {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write_u64(self.hash);
    }
}

impl std::fmt::Debug for Microcode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.words.iter()).finish()
    }
}

impl serde::Serialize for Microcode {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(self.words.iter())
    }
}

impl<'de> serde::Deserialize<'de> for Microcode {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Vec::<u32>::deserialize(d).map(Self::from)
    }
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
    /// Constant registers, hardware order (index = register + 96). Shared
    /// with the draws before while no register changes.
    pub constants: Constants,
    /// The 60-dword `D3DPIXELSHADERDEF`, or `None` for the fixed-function
    /// combiner D3D derives from the texture-stage states. Shared with the
    /// draw before when its words are the same.
    pub pixel_shader: Option<std::sync::Arc<[u32; 60]>>,
    /// Render-state slots 0..57 (`PS*`: combiner inputs/outputs/constants as
    /// the guest last wrote them; constants can change without a new PSDEF).
    /// Shared with the draw before when the words are the same.
    pub combiner_state: std::sync::Arc<[u32; 57]>,
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

    /// The draws prepared from now on go to a target `scale` times the
    /// guest's size (`crate::scaled`; 1, the default, is the guest's size).
    /// Window coordinates stay the guest's; the provider shifts positions
    /// by [`gfx::renderer::window_shift`] so edges cover what they cover
    /// at 1x.
    fn set_render_scale(&mut self, _scale: u32) {}

    /// E3 lighting's hard shadows (docs/graphics/e3-lighting.md, phase 1):
    /// from now on every shadow-map comparison is one tap
    /// (`shaders::psh::PixelOptions::hard_shadow`). Only a render-scale
    /// view is told this; off (the default) is retail's lookup.
    fn set_hard_shadows(&mut self, _on: bool) {}

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

#[cfg(test)]
mod tests {
    use super::Microcode;
    use std::hash::{BuildHasher, Hash};

    /// A recycled constant block comes back for the next draw instead of
    /// a new allocation.
    #[test]
    fn constant_blocks_are_reused() {
        let mut block = super::spare_constants();
        std::sync::Arc::get_mut(&mut block).expect("unshared")[7] = [1.0, 2.0, 3.0, 4.0];
        let at = &*block as *const _;
        // A block still held elsewhere is not kept.
        let held = super::spare_constants();
        let mut shared = vec![held.clone()];
        super::recycle_constants(&mut shared);
        assert!(shared.is_empty());
        assert_eq!(std::sync::Arc::strong_count(&held), 1);
        let mut spent = vec![block];
        super::recycle_constants(&mut spent);
        assert!(spent.is_empty());
        // Other tests may take or give blocks meanwhile: look for ours.
        let mut taken = Vec::new();
        let found = (0..super::SPARE_CONSTANT_BLOCKS).any(|_| {
            let b = super::spare_constants();
            let hit = std::ptr::eq(&*b, at);
            taken.push(b);
            hit
        });
        super::recycle_constants(&mut taken);
        assert!(found);
    }

    /// Shared microcode prints, serializes (so persisted program ids) and
    /// compares as the words it holds, shared or not.
    #[test]
    fn microcode_is_its_words() {
        let words = vec![0x1234_5678u32, 0, 7, 0xFFFF_FFFF];
        let a = Microcode::from(words.clone());
        let b = Microcode::from(words.clone());
        let c = Microcode::from(vec![1u32, 2, 3, 5]);
        assert!(!a.same(&b));
        assert!(a.same(&a.clone()));
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(*a, words[..]);
        assert_eq!(format!("{a:?}"), format!("{words:?}"));
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&words).unwrap()
        );
        assert_eq!(
            crate::pipeline_store::program_id(&a),
            crate::pipeline_store::program_id(&words)
        );
        let back: Microcode = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
        assert_eq!(back, a);
        let s = std::hash::RandomState::new();
        let h = |m: &Microcode| {
            let mut st = s.build_hasher();
            m.hash(&mut st);
            std::hash::Hasher::finish(&st)
        };
        assert_eq!(h(&a), h(&b));
        assert_eq!(h(&back), h(&a));
    }
}
