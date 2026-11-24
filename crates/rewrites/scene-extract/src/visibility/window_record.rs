//! The window written by 0x00163db0 at the portal walk's materialization
//! call. Offsets 4..16 and 32..36 belong to the caller.

use super::frusta::{self, Inputs};
use super::{Decline, guest, read};
use crate::GuestRead;

pub const SIZE: usize = 0x108;

/// Bitwise equality includes signed zero in every source float.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowInputs {
    ordinal: u32,
    rect: [u32; 4],
    world: [u32; 13],
    far: u32,
    clip: Option<[u32; 4]>,
    bounds: [u32; 6],
    unit: u32,
    minus: u32,
    epsilon: u32,
}

fn words<const N: usize>(mem: &dyn GuestRead, base: u32) -> Result<[u32; N], Decline> {
    let mut out = [0; N];
    for (i, word) in out.iter_mut().enumerate() {
        *word = read::u32_at(mem, base.checked_add(4 * i as u32).ok_or(Decline::Layout)?)
            .ok_or(Decline::Unread)?;
        if !f32::from_bits(*word).is_finite() {
            return Err(Decline::Frusta);
        }
    }
    Ok(out)
}

impl WindowInputs {
    pub fn read(
        mem: &dyn GuestRead,
        frustum: u32,
        rect: u32,
        ordinal: u32,
    ) -> Result<Self, Decline> {
        let field = |n| frustum.checked_add(n).ok_or(Decline::Layout);
        let [far] = words(mem, field(0x88)?)?;
        let clip = match read::u8_at(mem, field(0x6c)?).ok_or(Decline::Unread)? {
            0 => None,
            _ => Some(words(mem, field(0x74)?)?),
        };
        let bounds = read::ptr_at(mem, guest::FRUSTUM_BOUNDS).ok_or(Decline::Unread)?;
        Ok(Self {
            ordinal,
            rect: words(mem, rect)?,
            world: words(mem, field(0x38)?)?,
            far,
            clip,
            bounds: words(mem, bounds)?,
            unit: words::<1>(mem, guest::FRUSTUM_UNIT)?[0],
            minus: words::<1>(mem, guest::FRUSTUM_NEGATIVE_UNIT)?[0],
            epsilon: words::<1>(mem, guest::NORMALIZE_EPSILON)?[0],
        })
    }

    /// Fill exactly the defined record fields in the incoming slot image.
    /// False is the original leaf's AL=0 exit with the slot untouched.
    pub fn build(&self, slot: &mut [u8; SIZE]) -> Result<bool, Decline> {
        let r = self.rect.map(f32::from_bits);
        let [x0, x1, y0, y1] = r;
        if x1 <= x0 || y1 <= y0 {
            return Ok(false);
        }
        let world = self.world.map(f32::from_bits);
        let scale = world[0];
        let axes = [
            [world[1], world[2], world[3]],
            [world[4], world[5], world[6]],
            [world[7], world[8], world[9]],
        ];
        let t = [world[10], world[11], world[12]];
        let far = f32::from_bits(self.far);
        let inputs = Inputs {
            far,
            unit: f32::from_bits(self.unit),
            negative_unit: f32::from_bits(self.minus),
            epsilon: f32::from_bits(self.epsilon),
            bounds: self.bounds.map(f32::from_bits),
            clip_plane: self.clip.map(|p| p.map(f32::from_bits)),
        };
        let corners = [[x0, y1], [x0, y0], [x1, y1], [x1, y0]].map(|[x, y]| {
            let x = x * far;
            let y = y * far;
            let z = 0.0 - far;
            std::array::from_fn::<_, 3, _>(|k| {
                (x * (axes[0][k] * scale) + y * (axes[1][k] * scale))
                    + (z * (axes[2][k] * scale) + t[k])
            })
        });
        let mut bounds = inputs.bounds;
        for p in corners.iter().chain([&t]) {
            for k in 0..3 {
                if p[k] < bounds[2 * k] {
                    bounds[2 * k] = p[k];
                }
                if p[k] > bounds[2 * k + 1] {
                    bounds[2 * k + 1] = p[k];
                }
            }
        }
        let plane = |[x, y, z]: [f32; 3], d: f32, vertical: bool| {
            let n = [0, 1, 2].map(|k| (axes[2][k] * z + axes[1][k] * y) + axes[0][k] * x);
            // The bottom and top plane writers add the z term before the
            // scaled plane distance. The other four add it afterwards.
            let yz = if vertical {
                (t[2] * n[2] + t[1] * n[1]) + d * scale
            } else {
                (t[1] * n[1] + d * scale) + t[2] * n[2]
            };
            [n[0], n[1], n[2], yz + t[0] * n[0]]
        };
        let side = |v, vertical| plane(frusta::normalize(v, &inputs), 0.0, vertical);
        let planes = [
            side([inputs.negative_unit, 0.0, 0.0 - x0], false),
            side([inputs.unit, 0.0, x1], false),
            side([0.0, inputs.negative_unit, 0.0 - y0], true),
            side([0.0, inputs.unit, y1], true),
            match inputs.clip_plane {
                Some(p) => p.map(|v| 0.0 - v),
                None => plane([0.0, 0.0, inputs.unit], 0.0, false),
            },
            plane([0.0, 0.0, inputs.negative_unit], far, false),
        ];
        let rays = corners.map(|p| [0, 1, 2].map(|k| p[k] - t[k]));
        let tail = corners
            .into_iter()
            .flatten()
            .chain(t)
            .chain(bounds)
            .chain(planes.into_iter().flatten())
            .chain(rays.into_iter().flatten());
        slot[..4].copy_from_slice(&self.ordinal.to_le_bytes());
        for (out, value) in slot[0x10..0x20].chunks_exact_mut(4).zip(self.rect) {
            out.copy_from_slice(&value.to_le_bytes());
        }
        for (out, value) in slot[0x24..].chunks_exact_mut(4).zip(tail) {
            if !value.is_finite() {
                return Err(Decline::Frusta);
            }
            out.copy_from_slice(&value.to_le_bytes());
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn view() -> WindowInputs {
        WindowInputs {
            ordinal: 23,
            rect: [-2., 3., -1., 2.].map(f32::to_bits),
            world: [2., 1., 0., 0., 0., 1., 0., 0., 0., 1., 4., 3., 2.].map(f32::to_bits),
            far: 8f32.to_bits(),
            clip: None,
            bounds: [100., -100., 100., -100., 100., -100.].map(f32::to_bits),
            unit: 1f32.to_bits(),
            minus: (-1f32).to_bits(),
            epsilon: 0.001f32.to_bits(),
        }
    }

    fn floats<const N: usize>(slot: &[u8; SIZE], at: usize) -> [f32; N] {
        std::array::from_fn(|i| {
            f32::from_le_bytes(slot[at + 4 * i..at + 4 * i + 4].try_into().unwrap())
        })
    }

    #[test]
    fn writes_corners_bounds_planes_and_directions_preserving_caller_fields() {
        let mut slot = [0xa5; SIZE];
        assert_eq!(view().build(&mut slot), Ok(true));
        assert_eq!(slot[..4], 23u32.to_le_bytes());
        assert_eq!(&slot[4..16], &[0xa5; 12]);
        assert_eq!(&slot[0x20..0x24], &[0xa5; 4]);
        assert_eq!(floats(&slot, 0x24), [-28., 35., -14.]);
        assert_eq!(floats(&slot, 0x48), [52., -13., -14.]);
        assert_eq!(floats(&slot, 0x54), [4., 3., 2.]);
        assert_eq!(floats(&slot, 0x60), [-28., 52., -13., 35., -14., 2.]);
        assert_eq!(floats(&slot, 0xb8), [0., 0., 1., 2.]);
        assert_eq!(floats(&slot, 0xc8), [0., 0., -1., 14.]);
        assert_eq!(floats(&slot, 0xfc), [48., -16., -16.]);
    }

    #[test]
    fn explicit_clip_plane_is_in_world_space_and_empty_window_writes_nothing() {
        let mut view = view();
        let mut slot = [0x5a; SIZE];
        view.clip = Some([1., 2., 3., 4.].map(f32::to_bits));
        assert_eq!(view.build(&mut slot), Ok(true));
        assert_eq!(floats(&slot, 0xb8), [-1., -2., -3., -4.]);
        view.rect[1] = view.rect[0];
        let before = slot;
        assert_eq!(view.build(&mut slot), Ok(false));
        assert_eq!(before, slot);
    }
}
