//! Uniform scale, three basis columns and translation, as stored in a
//! structure instance. Keep the accumulation order of the three guest
//! routines: inverse (141590), local ray (1691a0), world plane (168b40).
use crate::{Bail, GuestRead};
use sim_core::det::{self, Fp};
use std::cmp::Ordering;

type V = [u32; 3];

#[derive(Clone, Copy)]
pub struct PlacementMatrix {
    scale: u32,
    basis: [V; 3],
    translation: V,
}

impl PlacementMatrix {
    pub fn read(g: &impl GuestRead, at: u32) -> Result<Self, Bail> {
        let mut bytes = [0; 52];
        g.read(at, &mut bytes)?;
        Ok(Self::from_words(std::array::from_fn(|i| {
            u32::from_le_bytes(bytes[4 * i..4 * i + 4].try_into().unwrap())
        })))
    }

    pub fn from_words(words: [u32; 13]) -> Self {
        Self {
            scale: words[0],
            basis: std::array::from_fn(|i| std::array::from_fn(|j| words[1 + 3 * i + j])),
            translation: [words[10], words[11], words[12]],
        }
    }

    pub fn words(&self) -> [u32; 13] {
        let mut words = [0; 13];
        words[0] = self.scale;
        words[1..10].copy_from_slice(self.basis.as_flattened());
        words[10..].copy_from_slice(&self.translation);
        words
    }

    pub fn inverse(&self, one: u32) -> Result<Self, Bail> {
        let mut fp = Fp::new();
        let inverted = self.invert(one, &mut fp);
        fp.exact().then_some(inverted).ok_or(Bail::Float)
    }

    fn invert(&self, one: u32, fp: &mut Fp) -> Self {
        fp.check(self.scale);
        fp.check(one);
        if det::compare(self.scale, 0) == Ordering::Equal {
            return Self {
                scale: 0,
                basis: [[0; 3]; 3],
                translation: [0; 3],
            };
        }
        let unit = det::compare(self.scale, one) == Ordering::Equal;
        let scale = if unit { one } else { fp.div(one, self.scale) };
        let t = self.translation.map(|v| {
            let neg = fp.sub(0, v);
            if unit { neg } else { fp.mul(scale, neg) }
        });
        let basis: [V; 3] = std::array::from_fn(|i| std::array::from_fn(|j| self.basis[j][i]));
        let translation = std::array::from_fn(|i| {
            let x = fp.mul(basis[0][i], t[0]);
            let z = fp.mul(basis[2][i], t[2]);
            let xz = fp.add(x, z);
            let y = fp.mul(basis[1][i], t[1]);
            fp.add(xz, y)
        });
        Self {
            scale,
            basis,
            translation,
        }
    }

    fn rotate(&self, v: V, fp: &mut Fp) -> V {
        std::array::from_fn(|i| {
            let z = fp.mul(self.basis[2][i], v[2]);
            let y = fp.mul(self.basis[1][i], v[1]);
            let zy = fp.add(z, y);
            let x = fp.mul(self.basis[0][i], v[0]);
            fp.add(zy, x)
        })
    }

    pub fn local_ray(&self, one: u32, start: V, delta: V) -> Result<(V, V), Bail> {
        let mut fp = Fp::new();
        let ray = self.local_ray_with(&mut fp, one, start, delta);
        if fp.exact() { Ok(ray) } else { Err(Bail::Float) }
    }

    /// [`PlacementMatrix::local_ray`] in the caller's `fp` ([`Fp::guest`]
    /// runs any operand and gathers the guest's flags).
    pub fn local_ray_with(&self, fp: &mut Fp, one: u32, start: V, delta: V) -> (V, V) {
        let inv = self.invert(one, fp);
        let mut transform = |mut v: V, point: bool| {
            if det::compare(inv.scale, one) != Ordering::Equal {
                v = v.map(|x| fp.mul(inv.scale, x));
            }
            let mut v = inv.rotate(v, &mut *fp);
            if point {
                for (x, t) in v.iter_mut().zip(inv.translation) {
                    *x = fp.add(*x, t);
                }
            }
            v
        };
        (transform(start, true), transform(delta, false))
    }

    pub fn plane(&self, p: [u32; 4], flipped: bool) -> Result<[u32; 4], Bail> {
        let mut fp = Fp::new();
        let out = self.plane_with(&mut fp, p, flipped);
        if fp.exact() { Ok(out) } else { Err(Bail::Float) }
    }

    /// [`PlacementMatrix::plane`] in the caller's `fp`.
    pub fn plane_with(&self, fp: &mut Fp, p: [u32; 4], flipped: bool) -> [u32; 4] {
        let [x, y, z] = self.rotate([p[0], p[1], p[2]], fp);
        let tz = fp.mul(self.translation[2], z);
        let scaled = fp.mul(p[3], self.scale);
        let mut d = fp.add(tz, scaled);
        let ty = fp.mul(self.translation[1], y);
        d = fp.add(d, ty);
        let tx = fp.mul(x, self.translation[0]);
        d = fp.add(d, tx);
        let mut out = [x, y, z, d];
        if flipped {
            out = out.map(|n| fp.sub(0, n));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: u32 = 1.0f32.to_bits();

    #[test]
    fn inverse_translation_uses_the_guest_accumulation_order() {
        // The two large terms cancel before the unit term is added.
        // Adding in column order loses the translation completely.
        let source = [
            1., 1., 1., 1., 0., 1., 0., 0., 0., 1., -33554432., -1., 33554432.,
        ]
        .map(f32::to_bits);
        let m = PlacementMatrix::from_words(source);
        let inv = m.inverse(ONE).unwrap().words();
        assert_eq!(inv[10], ONE);
        assert_eq!(&inv[1..10], &[ONE, 0, 0, ONE, ONE, 0, ONE, 0, ONE]);
        assert_eq!(m.words(), source);
    }

    #[test]
    fn zero_scale_consumes_no_basis_or_translation_operands() {
        let mut source = [f32::NAN.to_bits(); 13];
        source[0] = (-0.0f32).to_bits();
        let m = PlacementMatrix::from_words(source);
        assert_eq!(m.inverse(ONE).unwrap().words(), [0; 13]);
        assert_eq!(
            m.local_ray(ONE, [ONE; 3], [ONE; 3]).unwrap(),
            ([0; 3], [0; 3])
        );
        source[0] = f32::NAN.to_bits();
        assert!(matches!(
            PlacementMatrix::from_words(source).inverse(ONE),
            Err(Bail::Float)
        ));
    }

    #[test]
    fn flipped_plane_subtracts_from_zero_after_scaling_distance() {
        let m = PlacementMatrix::from_words(
            [2., 0., 1., 0., -1., 0., 0., 0., 0., 1., 3., 5., 2.].map(f32::to_bits),
        );
        let p = [ONE, 0, 0, 3.0f32.to_bits()];
        assert_eq!(
            m.plane(p, false).unwrap(),
            [0., 1., 0., 11.].map(f32::to_bits)
        );
        assert_eq!(
            m.plane(p, true).unwrap(),
            [0., -1., 0., -11.].map(f32::to_bits)
        );
    }
}
