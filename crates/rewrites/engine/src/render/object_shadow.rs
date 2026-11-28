//! The object-shadow gather and its lighting sample. The gather ranks up to
//! 32 objects; the projection reads the selected object's lighting separately.

use super::{DatumHandle, GuestMemory, MemFault, f32_at, u32_at, vec3_at};

/// A 20-byte row returned by the gather at `0x00033400`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    /// The local-player priority predicate (`0x0003e9c0`), including its camera test.
    pub local_player: bool,
    /// Key matched against the previous frame's history.
    pub history_key: DatumHandle,
    /// First argument to the projection at `0x00033647`.
    pub caster: DatumHandle,
    /// Second argument: after ranking, this is the history's current fade.
    pub fade: f32,
    /// Projected sphere radius used to rank this caster.
    pub priority: f32,
}

impl Candidate {
    /// `address` points to the start of the row, eight bytes before the
    /// projection loop's ESI cursor.
    pub fn read(m: &dyn GuestMemory, address: u32) -> Result<Self, MemFault> {
        let mut bytes = [0; 20];
        m.read(address, &mut bytes)?;
        Ok(Self {
            local_player: bytes[0] != 0,
            history_key: DatumHandle(u32_at(&bytes, 4)),
            caster: DatumHandle(u32_at(&bytes, 8)),
            fade: f32_at(&bytes, 12),
            priority: f32_at(&bytes, 16),
        })
    }
}

/// Fields consumed by the mask branch of `0x00032350`. The creature branch
/// uses its own ground projection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lighting {
    /// The projection normalizes this vector before constructing its basis.
    pub direction: [f32; 3],
    /// Multiplied by the current caster fade and the candidate's ranked fade.
    pub strength: f32,
}

impl Lighting {
    /// `sample` is the pointer returned by `0x0003dc90` (cache entry +0x54
    /// or default lighting +0x1c). Read through the last field used.
    pub fn read(m: &dyn GuestMemory, sample: u32) -> Result<Self, MemFault> {
        let mut bytes = [0; 0x20];
        m.read(sample, &mut bytes)?;
        Ok(Self {
            direction: vec3_at(&bytes, 0x0c),
            strength: f32_at(&bytes, 0x1c),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::FlatMemory;

    #[test]
    fn gathered_caster_has_separate_identity_fade_and_priority() {
        let mut m = FlatMemory::new(0x1100, 20);
        m.set_u32(0x1100, 0x0703_0201).unwrap();
        m.set_u32(0x1104, 0x8001_0002).unwrap();
        m.set_u32(0x1108, 0x8002_0007).unwrap();
        m.set_f32(0x110c, 0.375).unwrap();
        m.set_f32(0x1110, 14.5).unwrap();
        assert_eq!(
            Candidate::read(&m, 0x1100).unwrap(),
            Candidate {
                local_player: true,
                history_key: DatumHandle(0x8001_0002),
                caster: DatumHandle(0x8002_0007),
                fade: 0.375,
                priority: 14.5,
            }
        );
        assert!(Candidate::read(&m, 0x1101).is_err());
    }

    #[test]
    fn lighting_reads_the_projection_fields_from_the_resolved_sample() {
        let mut m = FlatMemory::new(0x2000, 0x20);
        for (offset, value) in [(12, -2.0), (16, 4.0), (20, 3.0), (24, 5.0), (28, 0.625)] {
            m.set_f32(0x2000 + offset, value).unwrap();
        }
        assert_eq!(
            Lighting::read(&m, 0x2000).unwrap(),
            Lighting { direction: [-2.0, 4.0, 3.0], strength: 0.625 }
        );
        assert!(Lighting::read(&m, 0x2001).is_err());
    }
}
