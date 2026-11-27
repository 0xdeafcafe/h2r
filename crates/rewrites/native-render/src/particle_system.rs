//! A particle system's data as the GPU holds it (docs/graphics/native-particles.md,
//! "GPU particle systems"). Pure: the `#[repr(C)]` records here are the
//! WGSL structs of `particle_common.wgsl` byte for byte, and
//! [`crate::particle_compute`] owns the buffers they live in.
//!
//! The evaluation mirrors `0x00224550` (`engine::particle_emission`): a
//! property is a tag function of one input and a range input, mapped to
//! its bounds and modified by another input (`0x00246cd0`); colour is a
//! function into a gradient (`0x00246d80`); alpha is scaled by the
//! system's tint, colour by the tint and, with flag `0x180`, the
//! particle's own; the sprite frame comes from a sequence and a frame
//! property, blended with the next. The tag function arrives baked as two
//! curves of [`CURVE`] samples (the function at range 0 and range 1),
//! which the evaluator interpolates; flag values are the definition's own.
//!
//! Particles are visual state only: nothing here reads or writes the sim
//! arena or enters its hash.

/// Threads per workgroup in every particle kernel.
pub const WORKGROUP: u32 = 256;
/// The most particles one system holds: the block scan is one workgroup
/// of [`WORKGROUP`] block totals.
pub const MAX_CAPACITY: u32 = WORKGROUP * WORKGROUP;
/// Samples in a baked function curve.
pub const CURVE: usize = 16;
/// Stops in a colour gradient.
pub const GRADIENT: usize = 8;
/// Values a property can read ([`Input`]).
pub const INPUTS: u32 = 8;
/// A modifier input of this value (or any not below [`INPUTS`]) is none.
pub const NO_INPUT: u32 = u32::MAX;

/// Definition flags (the guest definition's `+0` bits).
pub mod flag {
    /// Flip u when the particle's word bit 0 is set.
    pub const FLIP_U: u32 = 0x2;
    /// Flip v when the particle's word bit 16 is set.
    pub const FLIP_V: u32 = 0x4;
    /// The last frame of a sequence is not reached (frames less one).
    pub const FRAMES_LESS_ONE: u32 = 0x8;
    /// The sequence is picked by the particle's word (high half).
    pub const RANDOM_SEQUENCE: u32 = 0x10;
    /// No blend into the next frame.
    pub const NO_BLEND: u32 = 0x20;
    /// Colour tinted by the particle's packed tint.
    pub const PARTICLE_TINT: u32 = 0x180;
    /// Rotation offset by the definition's spin.
    pub const SPIN: u32 = 0x2000;
    /// The definition's own window and corners, no bitmap frames.
    pub const FIXED_WINDOW: u32 = 0x1_0000;
}

/// What a property reads: index into the particle's input vector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Input {
    /// Age over lifetime, 0 at birth.
    Life = 0,
    /// Age in seconds.
    Age = 1,
    /// The particle's four random lanes, fixed at birth.
    Random0 = 2,
    Random1 = 3,
    Random2 = 4,
    Random3 = 5,
    One = 6,
    Zero = 7,
}

/// Modifier operations (a property's `+4`).
pub mod modifier {
    pub const NONE: u32 = 0;
    pub const ADD: u32 = 1;
    pub const MULTIPLY: u32 = 2;
}

/// A scalar property (`0x00246cd0`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Property {
    pub input: u32,
    pub range: u32,
    pub modifier_input: u32,
    pub modifier: u32,
    /// The bounds the function's value maps to, when `map` is set.
    pub lo: f32,
    pub hi: f32,
    /// Non-zero: clamp the value to 0..1, then map it to `lo..hi`.
    pub map: u32,
    pub _pad: u32,
    /// The function at range 0 and range 1, sampled evenly over input 0..1.
    pub curve_lo: [[f32; 4]; CURVE / 4],
    pub curve_hi: [[f32; 4]; CURVE / 4],
}

impl Property {
    /// A property whose value is `v` everywhere.
    pub fn constant(v: f32) -> Self {
        Self::curve(Input::Zero, [v; CURVE])
    }

    /// A property reading `input` through `curve` at range 0 and 1 both.
    pub fn curve(input: Input, curve: [f32; CURVE]) -> Self {
        let c = pack(curve);
        Self {
            input: input as u32,
            range: Input::Zero as u32,
            modifier_input: NO_INPUT,
            modifier: modifier::NONE,
            lo: 0.0,
            hi: 1.0,
            map: 0,
            _pad: 0,
            curve_lo: c,
            curve_hi: c,
        }
    }
}

/// A colour property (`0x00246d80`): a function's value into a gradient.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ColourProperty {
    pub input: u32,
    pub range: u32,
    pub modifier_input: u32,
    pub modifier: u32,
    pub curve_lo: [[f32; 4]; CURVE / 4],
    pub curve_hi: [[f32; 4]; CURVE / 4],
    /// RGB stops (w unused) over the function's value 0..1.
    pub gradient: [[f32; 4]; GRADIENT],
}

/// One sprite of a bitmap sequence (the guest's 0x20-byte sprite).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Sprite {
    /// `(u0, u1, v0, v1)`.
    pub window: [f32; 4],
    /// The registration point, `(u, v)` within the window.
    pub registration: [f32; 2],
    pub _pad: [f32; 2],
}

/// A particle definition, the system's uniform.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Definition {
    pub flags: u32,
    /// The first sequence and how many the random pick spans.
    pub sequence_first: i32,
    pub sequence_pick: u32,
    /// Sprites per sequence in the sprite table; 0 means no bitmap.
    pub frames: u32,
    /// Sequences in the sprite table (`sprites.len() / frames`).
    pub sequences: u32,
    /// Rotation's scale (`0x0043f17c`), flag 0x2000's offset, size's scale.
    pub angle_scale: f32,
    pub spin: f32,
    pub size_scale: f32,
    /// `(alpha, r, g, b)`.
    pub tint: [f32; 4],
    /// World acceleration and linear drag per second.
    pub gravity: [f32; 3],
    pub drag: f32,
    /// Flag 0x10000's window `(u0, u1, v0, v1)` and corners.
    pub fixed_window: [f32; 4],
    pub fixed_corners: [f32; 4],
    pub rotation: Property,
    pub alpha: Property,
    pub size: Property,
    pub frame: Property,
    pub colour: ColourProperty,
}

/// A particle as the GPU holds it: its state, then what the update
/// evaluated from it for the draw.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Particle {
    pub position: [f32; 3],
    pub age: f32,
    pub velocity: [f32; 3],
    pub lifetime: f32,
    pub random: [f32; 4],
    pub id: u32,
    /// Bit 0 flips u, bit 16 flips v; the high half picks the sequence.
    pub words: u32,
    /// Packed RGB tint, red in the low byte.
    pub tint: u32,
    /// Evaluated: packed RGBA, red in the low byte (`unpack4x8unorm`).
    pub colour: u32,
    pub size: f32,
    pub rotation: f32,
    /// The blend into the next frame's window.
    pub blend: f32,
    pub _pad: f32,
    /// `(u, v, width, height)`, flips applied.
    pub window: [f32; 4],
    pub next_window: [f32; 4],
    /// The quad's extents `(x0, y0, x1, y1)` in size units.
    pub corners: [f32; 4],
}

/// A particle to be born at the next update, at `age` (0 for a new one;
/// a mirrored particle's own age).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Spawn {
    pub position: [f32; 3],
    pub lifetime: f32,
    pub velocity: [f32; 3],
    pub id: u32,
    pub random: [f32; 4],
    pub words: u32,
    pub tint: u32,
    pub age: f32,
    pub _pad: u32,
}

/// The update's uniform.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Step {
    pub dt: f32,
    pub spawns: u32,
    pub capacity: u32,
    /// Non-zero: no particle survives; the system is the births alone
    /// ([`crate::particle_compute::GpuParticles::mirror`]).
    pub mirror: u32,
}

/// The draw's view uniform: clip from world, the billboard axes, and the
/// W-buffer depth a [`crate::particle_compute::ParticleDraw::w_buffered`]
/// draw stores.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct View {
    /// Column-major.
    pub clip_from_world: [[f32; 4]; 4],
    pub right: [f32; 4],
    pub up: [f32; 4],
    /// The W-buffer value of a world position `p` is `w_row · (p, 1)`
    /// (the guest's `c[3]`, already in depth units).
    pub w_row: [f32; 4],
    /// `(full scale, clip min, clip max, flags)`: the stored depth is the
    /// W value held to the clip range, floored with [`depth::FLOOR`],
    /// over the full scale; out of range it is culled, or clamped with
    /// [`depth::CLAMP`]. Read only by the W-buffered draw.
    pub depth: [f32; 4],
}

impl View {
    /// The uniform's bytes.
    pub fn as_bytes(&self) -> &[u8] {
        bytemuck::bytes_of(self)
    }
}

/// [`View::depth`]'s flags.
pub mod depth {
    /// The integer depth format's floor.
    pub const FLOOR: u32 = 1;
    /// Out-of-range W is clamped to the clip range, not culled.
    pub const CLAMP: u32 = 2;
}

/// The system's counters on the GPU (`State` in WGSL).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Counts {
    /// Particles in the current buffer.
    pub count: u32,
    /// The count the last update started from.
    pub old: u32,
    /// Survivors and births of the last update.
    pub alive: u32,
    pub spawned: u32,
}

/// Byte offsets in the indirect-arguments buffer.
pub mod args {
    /// The next update's dispatch `(x, 1, 1)`.
    pub const UPDATE: u64 = 0;
    /// This update's scatter dispatch.
    pub const SCATTER: u64 = 16;
    /// `draw_indirect`: 6 vertices, one instance per particle.
    pub const DRAW: u64 = 32;
    pub const BYTES: u64 = 48;
}

/// Corners of the quad in draw order, the D3D diagonal (0,1,2), (0,2,3).
pub const QUAD: [[f32; 2]; 6] = [[0., 0.], [1., 0.], [1., 1.], [0., 0.], [1., 1.], [0., 1.]];

/// Sixteen samples as the uniform's four vectors.
pub fn pack(curve: [f32; CURVE]) -> [[f32; 4]; CURVE / 4] {
    std::array::from_fn(|i| std::array::from_fn(|j| curve[4 * i + j]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The WGSL structs' sizes (`particle_common.wgsl`); a mismatch reads
    /// every later field wrong on the GPU.
    #[test]
    fn records_have_the_wgsl_sizes() {
        assert_eq!(size_of::<Property>(), 160);
        assert_eq!(size_of::<ColourProperty>(), 272);
        assert_eq!(size_of::<Definition>(), 96 + 4 * 160 + 272);
        assert_eq!(size_of::<Particle>(), 128);
        assert_eq!(size_of::<Spawn>(), 64);
        assert_eq!(size_of::<Sprite>(), 32);
        assert_eq!(size_of::<View>(), 128);
        assert_eq!(MAX_CAPACITY, 65_536);
    }
}
