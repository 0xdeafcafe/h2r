//! Halo 1 scenario light fixture placements (H1 0x1fca28, 0x118e00).

use crate::{Error, Result};

pub const PLACEMENTS: (usize, usize) = (0x2c4, 0x58);
pub const PALETTE: (usize, usize) = (0x2d0, 0x30);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lighting {
    pub rgb: [f32; 3],
    pub intensity: f32,
}

impl Lighting {
    /// The placement's RGB and intensity. A zero tuple leaves the
    /// fixture's own light definition in use.
    pub fn read(placement: &[u8]) -> Result<Option<Self>> {
        let fields = placement
            .get(0x30..0x40)
            .ok_or(Error::Invalid("truncated light fixture placement"))?;
        // Halo 1 also copies two following words (0x118e49..0x118e57).
        // Retail placements leave them zero. Their nonzero forms need a
        // source mapping before they can be written to the light.
        let extra = placement
            .get(0x40..0x48)
            .ok_or(Error::Invalid("truncated light fixture placement"))?;
        if extra != [0; 8] {
            return Err(Error::Invalid(
                "unsupported light fixture placement fields at +0x40",
            ));
        }
        let words = std::array::from_fn::<_, 4, _>(|i| {
            f32::from_le_bytes(fields[i * 4..i * 4 + 4].try_into().expect("4 bytes"))
        });
        let [r, g, b, intensity] = words;
        if words.iter().any(|f| !f.is_finite() || *f < 0.0) {
            return Err(Error::Invalid("invalid fixture colour or intensity"));
        }
        Ok((words != [0.0; 4]).then_some(Self {
            rgb: [r, g, b],
            intensity,
        }))
    }

    pub fn key(self) -> [u32; 4] {
        let [r, g, b] = self.rgb;
        [r, g, b, self.intensity].map(f32::to_bits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_placement_distinguishes_unauthored_and_dark() {
        let mut row = [0; PLACEMENTS.1];
        assert_eq!(Lighting::read(&row).unwrap(), None);
        row[0x34..0x38].copy_from_slice(&0.6f32.to_le_bytes());
        assert_eq!(
            Lighting::read(&row).unwrap(),
            Some(Lighting {
                rgb: [0.0, 0.6, 0.0],
                intensity: 0.0
            })
        );
        row[0x3c..0x40].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(Lighting::read(&row).is_err());
    }
}
