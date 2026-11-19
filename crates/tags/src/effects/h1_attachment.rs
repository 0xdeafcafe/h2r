//! Extra attachment simulation data in ordinary Halo 2 effects and movement
//! tags. The stock engine can still dispatch their particle and collision
//! blocks. The auxiliary words are read only by the Halo 1 particle hooks.

/// Discriminator in `effe +0x28` for a Halo 1 attachment population clock.
pub const POPULATION_CLOCK: u32 = u32::from_le_bytes(*b"H1PC");
/// Discriminator in the otherwise unused words of the first collision curve.
pub const POINT_PHYSICS: u32 = u32::from_le_bytes(*b"H1PP");

/// Discriminators in the native emitter speed curve's auxiliary words.
pub const RADIAL_BIRTH: u32 = u32::from_le_bytes(*b"H1R1");
pub const DIRECTIONAL_BIRTH: u32 = u32::from_le_bytes(*b"H1R2");

pub const KILL_IN_LIQUID: u32 = 0x10;
pub const KILL_IN_AIR: u32 = 0x20;
pub const KILL_ON_CONTACT: u32 = 0x40;
pub const MASS_BY_RADIUS: u32 = 0x20;

/// Point physics accompanies the two native constant collision parameters.
/// A scalar constant has no ranged set, and the evaluator reads only the
/// header range. Each parameter retains four auxiliary words at +0x0c.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointPhysics {
    pub flags: u32,
    pub particle_type_flags: u32,
    pub mass: f32,
    pub acceleration: [f32; 2], // air, liquid
    pub drag: [f32; 2],
    pub radius: f32,
}

impl PointPhysics {
    pub fn to_words(self) -> [[u32; 4]; 2] {
        [
            [
                POINT_PHYSICS,
                (self.flags & 0xffff) | (self.particle_type_flags & 0xffff) << 16,
                self.mass.to_bits(),
                self.radius.to_bits(),
            ],
            [
                self.acceleration[0].to_bits(),
                self.acceleration[1].to_bits(),
                self.drag[0].to_bits(),
                self.drag[1].to_bits(),
            ],
        ]
    }

    pub fn from_words(words: [[u32; 4]; 2]) -> Option<Self> {
        let [
            [magic, flags, mass, radius],
            [air_z, liquid_z, air_drag, liquid_drag],
        ] = words;
        if magic != POINT_PHYSICS {
            return None;
        }
        let p = Self {
            flags: flags & 0xffff,
            particle_type_flags: flags >> 16,
            mass: f32::from_bits(mass),
            radius: f32::from_bits(radius),
            acceleration: [air_z, liquid_z].map(f32::from_bits),
            drag: [air_drag, liquid_drag].map(f32::from_bits),
        };
        [
            p.mass,
            p.radius,
            p.acceleration[0],
            p.acceleration[1],
            p.drag[0],
            p.drag[1],
        ]
        .into_iter()
        .all(f32::is_finite)
        .then_some(p)
    }
}

/// Each endpoint in a phase's `effe` event word is Q7.8 particles.
/// Halo 1's retail attachment thresholds are integers below 128.
pub fn population_word(bounds: [f32; 2], scale_with_primary: bool) -> u32 {
    let [a, b] = bounds.map(|v| (v * 256.0).round().clamp(0.0, 32767.0) as u32);
    a | (b << 16) | (u32::from(scale_with_primary) << 31)
}

pub fn minimum_population(word: u32, progress: f32, primary: f32) -> f32 {
    let a = (word & 0x7fff) as f32 / 256.0;
    let b = ((word >> 16) & 0x7fff) as f32 / 256.0;
    let threshold = a + (b - a) * progress.clamp(0.0, 1.0);
    if word >> 31 == 0 {
        threshold
    } else {
        threshold * primary
    }
}
