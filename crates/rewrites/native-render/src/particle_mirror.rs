//! A guest particle as a GPU particle system's birth (docs/graphics/retire-d3d-r2.md,
//! "R2's particle mirror"). The guest owns the particle; each frame its
//! state at the callback `0x00224550` becomes a [`Spawn`] at its own age,
//! and the system is those births alone
//! ([`crate::particle_compute::GpuParticles::mirror`]). Pure.
//!
//! | Guest | [`Spawn`] |
//! |---|---|
//! | the callback's `position` | `position`; `velocity` zero (nothing integrates) |
//! | input 1 (age) | `age` |
//! | input 0 (life, age over lifetime) | `lifetime` = age / life; life 0 is an unbounded lifetime |
//! | inputs 2, 3, 14, 15 | `random` (`particle_definition::INPUT_LANES`) |
//! | particle `+0x14` (u32) | `words`: bit 0 flips u, `+0x16` bit 0 flips v, `+0x16` picks the sequence |
//! | particle `+0x3c`, red in bits 16..23 | `tint`, red in the low byte |
//!
//! The callback's `tint` and `size` arguments are the system's
//! (`Definition::tint`, `size_scale`), so a [`Key`] groups the particles
//! one system draws.

use crate::particle_system::{Definition, Spawn, flag};

/// The particles one system draws together: one definition and bitmap,
/// one callback `tint` and `size` (their bits).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub definition: u32,
    pub bitmap: u32,
    pub tint: [u32; 4],
    pub size: u32,
}

/// Why a particle is not mirrored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// `0x0023ea80`'s program class (definition `+4`) is not 0, the one
    /// whose quad faces the view as `ParticleDraw` builds it.
    Orientation(u16),
    /// Flag `0x180`'s tint scale (`BYTE` times `TINT`) is not 1/255, what
    /// `unpack4x8unorm` reads.
    TintScale,
}

impl Refused {
    /// `scene_kept_15_particle_mirror_<name>`.
    pub fn name(self) -> &'static str {
        match self {
            Refused::Orientation(_) => "orientation",
            Refused::TintScale => "tint_scale",
        }
    }
}

/// Whether a particle of a definition with `flags` and program class
/// `class` has a mirror. `byte` and `tint` are the image's `0x0045dbb4`
/// and `0x0045dc0c`.
pub fn check(flags: u32, class: u16, byte: f32, tint: f32) -> Result<(), Refused> {
    if class != 0 {
        return Err(Refused::Orientation(class));
    }
    if flags & flag::PARTICLE_TINT != 0 && byte * tint != 1.0 / 255.0 {
        return Err(Refused::TintScale);
    }
    Ok(())
}

/// The birth for a particle with the cached `inputs` of its block, at
/// `position`, with its `+0x14` `words` and `+0x3c` packed tint. `id` is
/// the particle's address.
pub fn spawn(inputs: &[f32; 16], position: [f32; 3], words: u32, packed: u32, id: u32) -> Spawn {
    let (life, age) = (inputs[0], inputs[1]);
    let lifetime = if life > 0.0 { age / life } else { f32::INFINITY };
    Spawn {
        position,
        lifetime,
        velocity: [0.0; 3],
        id,
        random: [inputs[2], inputs[3], inputs[14], inputs[15]],
        words,
        tint: ((packed >> 16) & 0xff) | (packed & 0xff00) | ((packed & 0xff) << 16),
        age,
        _pad: 0,
    }
}

/// `definition` as the system of `key` draws it: the callback's tint and
/// size scale, and no movement.
pub fn owned(definition: &Definition, key: &Key) -> Definition {
    Definition {
        tint: key.tint.map(f32::from_bits),
        size_scale: f32::from_bits(key.size),
        gravity: [0.0; 3],
        drag: 0.0,
        ..*definition
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_particle_becomes_a_birth_at_its_own_age_with_its_lanes_and_tint() {
        let mut inputs = [0.0; 16];
        inputs[0] = 0.25;
        inputs[1] = 1.5;
        inputs[2] = 0.1;
        inputs[3] = 0.2;
        inputs[14] = 0.3;
        inputs[15] = 0.4;
        let s = spawn(&inputs, [1.0, 2.0, 3.0], 0x0003_0001, 0x00aa_bbcc, 0x4c_1000);
        assert_eq!(s.age, 1.5);
        assert_eq!(s.lifetime, 6.0, "life is age over lifetime");
        assert_eq!(s.random, [0.1, 0.2, 0.3, 0.4], "inputs 2, 3, 14, 15");
        assert_eq!(s.tint, 0x00cc_bbaa, "red moves to the low byte");
        assert_eq!((s.words, s.id, s.velocity), (0x0003_0001, 0x4c_1000, [0.0; 3]));
    }

    #[test]
    fn a_particle_at_life_zero_has_an_unbounded_lifetime_so_its_life_reads_zero() {
        let s = spawn(&[0.0; 16], [0.0; 3], 0, 0, 1);
        assert_eq!((s.age, s.lifetime), (0.0, f32::INFINITY));
        assert_eq!(s.age / s.lifetime, 0.0);
    }

    #[test]
    fn only_a_view_facing_class_and_an_exact_tint_scale_mirror() {
        let byte = 1.0 / 255.0;
        assert_eq!(check(0, 0, byte, 1.0), Ok(()));
        assert_eq!(check(0, 2, byte, 1.0), Err(Refused::Orientation(2)));
        assert_eq!(check(flag::PARTICLE_TINT, 0, byte, 2.0), Err(Refused::TintScale));
        assert_eq!(check(0, 0, byte, 2.0), Ok(()), "the scale matters only with the particle's tint");
    }

    #[test]
    fn the_system_takes_the_callbacks_tint_and_size_and_does_not_move() {
        let d = Definition { gravity: [0.0, 0.0, -9.8], drag: 0.5, ..bytemuck::Zeroable::zeroed() };
        let key = Key { definition: 1, bitmap: 2, tint: [0.5f32, 1.0, 0.25, 0.75].map(f32::to_bits), size: 2.0f32.to_bits() };
        let o = owned(&d, &key);
        assert_eq!((o.tint, o.size_scale, o.gravity, o.drag), ([0.5, 1.0, 0.25, 0.75], 2.0, [0.0; 3], 0.0));
    }
}
