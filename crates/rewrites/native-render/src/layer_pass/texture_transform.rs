//! Texture-slot properties consumed by vertex binding modes 5 through 20.

/// The bitmap binding at `0x00017550` writes the components of the slot
/// independently. `0x00016f60` sets its frame and LOD, and leaves these
/// transform fields in place. A field missing from this draw is unknown.
#[derive(Default)]
pub(super) struct TextureTransforms {
    slots: [[Option<f32>; 7]; 8],
}

impl TextureTransforms {
    /// Record a scale (components 1..3), offset (4..6) or rotation (7).
    pub(super) fn set(&mut self, slot: u8, component: i16, value: f32) -> Option<()> {
        let fields = self.slots.get_mut(usize::from(slot))?;
        let field = (1..=7).contains(&component).then(|| (component - 1) as usize)?;
        fields[field] = Some(value);
        Some(())
    }

    /// Expand this slot's transform into a vertex register (`0x0001b790`).
    /// Return the component mask. An unknown input needed for this write
    /// leaves the entire row untouched.
    pub(super) fn write(&self, slot: u8, mode: u8, row: &mut [f32; 4]) -> Option<u8> {
        let &[sx, sy, sz, ox, oy, oz, turns] = self.slots.get(usize::from(slot))?;
        let trig = turns.map(|turns| {
            if turns.abs() < 0.0001 {
                (0.0, 1.0)
            } else {
                (turns * std::f32::consts::TAU).sin_cos()
            }
        });
        let rx = || Some(sx? * trig?.1);
        let ry = || Some(sy? * trig?.1);
        let tx = || Some(((1.0 - trig?.1) + trig?.0) * sx? * 0.5 + ox?);
        let ty = || Some(((1.0 - trig?.1) - trig?.0) * sy? * 0.5 + oy?);
        let mask = match mode {
            5..=8 => {
                row[usize::from(mode - 5)] = rx()?;
                1 << (mode - 5)
            }
            9 | 10 => {
                let start = if mode == 9 { 0 } else { 2 };
                row[start..start + 2].copy_from_slice(&[rx()?, ry()?]);
                3 << start
            }
            11 | 12 => {
                let start = if mode == 11 { 0 } else { 2 };
                row[start..start + 2].copy_from_slice(&[tx()?, ty()?]);
                3 << start
            }
            13 => {
                *row = [rx()?, ry()?, tx()?, ty()?];
                15
            }
            14 | 18 => {
                *row = [rx()?, 0.0 - sx? * trig?.0, 0.0, tx()?];
                15
            }
            15 | 19 => {
                *row = [sy? * trig?.0, ry()?, 0.0, ty()?];
                15
            }
            16 => {
                row[..3].copy_from_slice(&[rx()?, ry()?, sz?]);
                7
            }
            17 => {
                row[..3].copy_from_slice(&[tx()?, ty()?, oz?]);
                7
            }
            20 => {
                *row = [0.0, 0.0, sz?, oz?];
                15
            }
            _ => return None,
        };
        Some(mask)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_projects_only_the_requested_fields_of_the_selected_slot() {
        let mut t = TextureTransforms::default();
        for (component, value) in [(1, 3.0), (2, 6.0), (4, 1.0), (5, 2.0), (7, 0.25)] {
            t.set(3, component, value).unwrap();
        }
        let mut row = [8.0; 4];
        assert_eq!(t.write(2, 12, &mut row), None);
        assert_eq!(t.write(3, 16, &mut row), None, "missing Z scale");
        assert_eq!(row, [8.0; 4]);
        assert_eq!(t.write(3, 12, &mut row), Some(12));
        assert_eq!(row[..2], [8.0; 2]);
        assert!((row[2] - 4.0).abs() < 1e-5 && (row[3] - 2.0).abs() < 1e-5);
        assert_eq!(t.write(3, 14, &mut row), Some(15));
        assert!(row[0].abs() < 1e-5 && (row[1] + 3.0).abs() < 1e-5);
        assert_eq!(row[2], 0.0);
        assert!((row[3] - 4.0).abs() < 1e-5);
    }

    #[test]
    fn the_z_row_uses_only_z_scale_and_z_offset() {
        let mut t = TextureTransforms::default();
        assert_eq!(t.set(0, 0, 42.0), None, "frame is separate");
        t.set(0, 3, 7.0).unwrap();
        t.set(0, 6, 9.0).unwrap();
        t.set(0, 6, 12.0).unwrap();
        let mut row = [8.0; 4];
        assert_eq!(t.write(0, 20, &mut row), Some(15));
        assert_eq!(row, [0.0, 0.0, 7.0, 12.0]);
        assert_eq!(t.write(0, 7, &mut row), None, "missing rotation and X scale");
    }
}
