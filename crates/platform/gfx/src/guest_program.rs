//! What the guest's shader programs are, as translation inputs.
//!
//! The vertex program (microcode), the pixel combiner definition and the
//! constants a translated draw consumes ([`ShaderInputs`]), and the pure
//! rules the translator applies to them: the combiner words, the fog
//! unit's parameters, the alpha comparison and the sampler key. No NV2A
//! register or push-buffer type crosses: a caller hands in the values.

use crate::state::CompareFunc;
#[cfg(feature = "gpu")]
use crate::state::TextureStageState;
#[cfg(feature = "gpu")]
use crate::translate::SamplerKey;
use shaders::psh::{self, AlphaFunc, PixelShaderDef, argb};

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
    ends_at_first_final: bool,
}

impl Microcode {
    /// Shares `words`, hashing them once.
    pub fn new(words: std::sync::Arc<[u32]>) -> Self {
        let hash = Self::hash_words(&words);
        let ends_at_first_final = !words.is_empty() && words.len().is_multiple_of(4)
            && words.chunks_exact(4).position(|i| i[3] & 1 != 0) == Some(words.len() / 4 - 1);
        Self { words, hash, ends_at_first_final }
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

    /// Whether these words have exactly the extent selected by NV2A
    /// program lookup. Check once when interning a prepared BSP program.
    pub fn ends_at_first_final(&self) -> bool {
        self.ends_at_first_final
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
    /// from z instead of w (`0x3f6f40`; `PassthroughFog`).
    pub affine_projection: bool,
}

/// The pixel translator's comparison for the device alpha-test state.
pub fn alpha_func(f: CompareFunc) -> AlphaFunc {
    match f {
        CompareFunc::Never => AlphaFunc::Never,
        CompareFunc::Less => AlphaFunc::Less,
        CompareFunc::Equal => AlphaFunc::Equal,
        CompareFunc::LessEqual => AlphaFunc::LessEqual,
        CompareFunc::Greater => AlphaFunc::Greater,
        CompareFunc::NotEqual => AlphaFunc::NotEqual,
        CompareFunc::GreaterEqual => AlphaFunc::GreaterEqual,
        CompareFunc::Always => AlphaFunc::Always,
    }
}

/// The guest's combiner program: render-state slots 0..57, texture modes
/// from slot 136, the last three dwords from the definition.
///
/// Whether there is a final combiner comes from the definition itself, as
/// D3D latches it at `SetPixelShader` (`0x3fa452`, device `+0x788`) and the
/// lazy commit reads the latch, not the definition: `final_latch` is that
/// value ([`ShaderInputs::final_combiner_latch`]). A definition bound with
/// a final combiner keeps it even when zero is later pushed to both
/// final-combiner methods ([`PixelShaderDef::explicit_final`]), and editing
/// the definition's buffer without rebinding changes neither way.
///
/// [`ShaderInputs::final_combiner_latch`]: ShaderInputs::final_combiner_latch
pub fn guest_def(
    psdef: &[u32; 60],
    combiner_state: &[u32; 57],
    texture_modes: u32,
    final_latch: bool,
) -> PixelShaderDef {
    let words = combiner_words(psdef, combiner_state, texture_modes);
    let mut def = PixelShaderDef::from_words(&words);
    def.explicit_final = final_latch;
    def
}

pub fn combiner_words(psdef: &[u32; 60], states: &[u32; 57], modes: u32) -> [u32; 60] {
    let mut words = *psdef;
    words[..57].copy_from_slice(states);
    words[54] = modes;
    words
}

/// The exp fog scale per unit density, `−1/(16 ln 2)`, so that
/// `2^(16·scale·d)` is `e^(−density·d)`. D3D multiplies by its own
/// constant (`0x45E378`), which differs from this by about 4e-6 relative;
/// the ignored `tests/fog_xbe.rs` reads it from `$H2_XBE` and checks the
/// tolerance.
pub fn exp_fog_scale() -> f32 {
    (-1.0 / (16.0 * std::f64::consts::LN_2)) as f32
}

/// The exp2 fog scale per unit density, `−1/√(32 ln 2)`, so that
/// `2^(−32·(scale·d)²)` is `e^(−(density·d)²)`. D3D's own constant
/// (`0x45E374`) differs from this by a similar few parts per million.
pub fn exp2_fog_scale() -> f32 {
    (-1.0 / (32.0 * std::f64::consts::LN_2).sqrt()) as f32
}

/// `NV097_SET_FOG_GEN_MODE` values D3D pushes.
pub mod fog_gen {
    /// `FOGGEN_SPEC_ALPHA`: the specular alpha (no fog table).
    pub const SPEC_ALPHA: u32 = 0;
    /// `FOGGEN_RADIAL`: the eye-space distance (`D3DRS_RANGEFOGENABLE`).
    pub const RADIAL: u32 = 1;
    /// `FOGGEN_PLANAR`: the eye-space depth.
    pub const PLANAR: u32 = 2;
}

/// The fog state D3D's lazy commit pushes (`0x00400000`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct D3dFog {
    /// `NV097_SET_FOG_GEN_MODE` ([`fog_gen`]). It selects the coordinate
    /// of the fixed-function transform only: vertex programs, D3D's
    /// pass-through programs among them, supply the coordinate in `oFog`, so no
    /// draw this provider translates reads it.
    pub gen_mode: u32,
    /// `NV097_SET_FOG_ENABLE`, `NV097_SET_FOG_MODE` and the bias and scale
    /// of `NV097_SET_FOG_PARAMS`.
    pub fog: psh::fog::Fog,
}

/// D3D's fog commit (`0x00400000`), from `D3DRS_FOGENABLE`,
/// `D3DRS_FOGTABLEMODE`, `D3DRS_FOGSTART`, `D3DRS_FOGEND`,
/// `D3DRS_FOGDENSITY` (the float states as their raw words) and
/// `D3DRS_RANGEFOGENABLE`:
///
/// * fog off: only `SET_FOG_ENABLE` 0 is pushed. The other registers keep
///   their old values, which no longer matter (the factor reads 1); this
///   returns [`psh::fog::Fog::OFF`] and `SPEC_ALPHA`;
/// * `D3DFOG_NONE`: `LINEAR` with bias 1 and scale 1, so the factor is the
///   vertex's `oFog.x`, and generation mode `SPEC_ALPHA`;
/// * `D3DFOG_LINEAR` (3): scale `−1/(end − start)` and bias
///   `1 + end/(end − start)`, or scale `−8192` (`0x45DFF4`) when start and
///   end compare equal (the x87 compare is unordered for NaN, which
///   divides);
/// * `D3DFOG_EXP` (1): bias 1.5 and scale `density ·` [`exp_fog_scale`];
///   any other mode, `D3DFOG_EXP2` (2) included: bias 1.5 and scale
///   `density ·` [`exp2_fog_scale`] (D3D's own constants differ from these
///   closed forms by about 4e-6 relative);
/// * with a table mode the generation mode is `RADIAL` when
///   `D3DRS_RANGEFOGENABLE` is set and `PLANAR` otherwise.
///
/// The x87 code keeps intermediates in extended precision and rounds when
/// it stores each parameter; here intermediates are `f64`.
pub fn d3d_fog(
    enable: bool,
    table_mode: u32,
    start: u32,
    end: u32,
    density: u32,
    range_fog: bool,
) -> D3dFog {
    use psh::fog::{Fog, FogMode};
    let f = |w: u32| f32::from_bits(w) as f64;
    let table_gen = if range_fog {
        fog_gen::RADIAL
    } else {
        fog_gen::PLANAR
    };
    let (gen_mode, fog) = match table_mode {
        _ if !enable => (fog_gen::SPEC_ALPHA, Fog::OFF),
        0 => (fog_gen::SPEC_ALPHA, Fog::FROM_VERTEX),
        3 => {
            let (start, end) = (f(start), f(end));
            let k = if start == end {
                8192.0
            } else {
                1.0 / (end - start)
            };
            let fog = Fog {
                enable: true,
                mode: FogMode::Linear,
                bias: (end * k + 1.0) as f32,
                scale: -k as f32,
            };
            (table_gen, fog)
        }
        m => {
            let (mode, k) = if m == 1 {
                (FogMode::Exp, exp_fog_scale())
            } else {
                (FogMode::Exp2, exp2_fog_scale())
            };
            let fog = Fog {
                enable: true,
                mode,
                bias: 1.5,
                scale: (f(density) * k as f64) as f32,
            };
            (table_gen, fog)
        }
    };
    D3dFog { gen_mode, fog }
}

/// The fog uniform and `FOGCOLOR` a draw's combiners read, from the draw's
/// own render-state words: [`d3d_fog`] for `enable`, the table mode, the
/// start, end and density words (the float states as their raw words) and
/// `range_fog`, and [`argb`] for `fog_color`.
pub fn draw_fog(
    enable: bool,
    table_mode: u32,
    start: u32,
    end: u32,
    density: u32,
    range_fog: bool,
    fog_color: u32,
) -> ([f32; 4], [f32; 4]) {
    (
        d3d_fog(enable, table_mode, start, end, density, range_fog).fog.uniform(),
        argb(fog_color),
    )
}

/// [`PixelOptions::border`] for a stage: the `BORDER` axes when the stage
/// reads signed channels, uses a non-preset colour, or the host cannot
/// clamp to border. The shader path approximates linear filtering at edges.
///
/// [`PixelOptions::border`]: shaders::psh::PixelOptions
pub fn shader_border(s: &TextureStageState, sign: u8, clamp_to_border: bool) -> u8 {
    use crate::state::TextureAddress;
    if sign == 0 && clamp_to_border && matches!(s.border_color, 0 | 0xff00_0000 | 0xffff_ffff) {
        return 0;
    }
    [s.address_u, s.address_v, s.address_w]
        .iter()
        .enumerate()
        .fold(0, |m, (axis, &a)| m | u8::from(a == TextureAddress::Border) << axis)
}

/// The register combiners and their constants at a draw, suitable for
/// carrying in a scene overlay: `pixel_shader` is the bound definition,
/// `combiner_state` the render-state words 0..57, `texture_modes` the
/// `PS_TEXTURE_MODES` word, `final_latch` the bound definition's final
/// combiner latch ([`ShaderInputs::final_combiner_latch`]), `fog` and
/// `specular` the render states, `stippled` whether the draw is stippled
/// (nothing is carried then). `None` when no definition is bound, the draw
/// is stippled, or a programmed zero final combiner cannot be told from an
/// absent one.
pub fn overlay_shader_words(
    pixel_shader: Option<&[u32; 60]>,
    combiner_state: &[u32; 57],
    texture_modes: u32,
    final_latch: bool,
    fog: bool,
    specular: bool,
    stippled: bool,
) -> Option<[u32; 60]> {
    if stippled {
        return None;
    }
    let mut words = combiner_words(pixel_shader?, combiner_state, texture_modes);
    if words[8] == 0 && words[9] == 0 {
        if final_latch {
            return None;
        }
        (words[8], words[9]) = psh::d3d_default_final_combiner(fog, specular);
    }
    Some(words)
}

#[cfg(feature = "gpu")]
/// Sampler paired with the border-axis mask from [`shader_border`].
pub fn sampler_key(
    stage: &TextureStageState,
    caps: gfx::translate::Capabilities,
    shader_border: u8,
) -> SamplerKey {
    let mut key = SamplerKey::from_stage(stage, caps);
    if shader_border != 0 {
        // The shader supplies the border; the sampler clamps.
        for (axis, mode) in [&mut key.address_u, &mut key.address_v, &mut key.address_w]
            .into_iter()
            .enumerate()
        {
            if shader_border >> axis & 1 != 0 {
                *mode = wgpu::AddressMode::ClampToEdge;
            }
        }
        key.border = None;
    }
    key
}

/// Whether `rows` (a 4x4 matrix) is a perspective projection.
pub fn is_perspective(rows: &[[f32; 4]]) -> bool {
    let m = std::array::from_fn(|i| rows[i].map(f64::from));
    crate::guest_projection::camera::Persp::split(&m).is_some()
}

/// Widen the lens of a captured perspective matrix, leaving orthographic
/// matrices intact: a `factor` below 1 scales the horizontal row, one above
/// 1 scales the vertical row by its inverse (`canvas_fit::Lens`).
pub fn widen_projection_rows(rows: &mut [[f32; 4]], factor: f32) -> bool {
    if factor == 1.0
        || !factor.is_finite()
        || factor <= 0.0
        || rows.len() != 4
        || !is_perspective(rows)
    {
        return false;
    }
    let (x, y) = (canvas_fit::horizontal_term(factor), canvas_fit::vertical_term(factor));
    rows[0] = rows[0].map(|v| v * x);
    rows[1] = rows[1].map(|v| v * y);
    true
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

    #[cfg(feature = "gpu")]
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

    #[test]
    fn overlay_reads_live_registers_and_keeps_definition_mappings() {
        let stored: [u32; 60] = std::array::from_fn(|i| 2000 + i as u32);
        let live: [u32; 57] = std::array::from_fn(|i| 5000 + i as u32);
        let words = super::combiner_words(&stored, &live, 73);
        assert_eq!(words[..54], live[..54]);
        assert_eq!(words[54], 73);
        assert_eq!(words[55..57], live[55..]);
        assert_eq!(words[57..], stored[57..]);
        assert_eq!(super::overlay_shader_words(Some(&stored), &live, 73, true, false, false, false), Some(words));
        assert!(super::overlay_shader_words(Some(&stored), &live, 73, true, false, false, true).is_none());
        assert!(super::overlay_shader_words(None, &live, 73, true, false, false, false).is_none());
    }

    #[test]
    fn overlay_final_observes_the_bind_time_latch() {
        let (stored, live) = ([0u32; 60], [0u32; 57]);
        assert!(super::overlay_shader_words(Some(&stored), &live, 0, true, false, false, false).is_none());
        let defaulted = super::overlay_shader_words(Some(&stored), &live, 0, false, false, false, false).unwrap();
        assert_eq!((defaulted[8], defaulted[9]), shaders::psh::d3d_default_final_combiner(false, false));
    }
}
