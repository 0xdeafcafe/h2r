//! The segment–sphere pretest used by the object collision query.
use crate::Bail;
use sim_core::det::{self, Fp};
use std::cmp::Ordering;

/// Sphere coordinates and radius as binary32 bit patterns.
pub struct Sphere {
    pub center: [u32; 3],
    pub radius: u32,
}

/// The start relative to the sphere. The intermediate values also let
/// the guest adapter replay the scratch registers.
pub struct SphereOffset {
    pub relative: [u32; 3],
    pub radius_squared: u32,
    pub gap: u32,
}

pub struct Sweep {
    pub projection: u32,
    pub x_product: u32,
    pub quadratic: Option<Quadratic>,
}

pub struct Quadratic {
    pub length_squared: u32,
    pub scaled_gap: u32,
    pub discriminant: u32,
    pub end: Option<End>,
}

pub struct End {
    pub gap: u32,
    pub gap_squared: Option<u32>,
}

impl Sphere {
    /// The start test comes first. A start strictly inside accepts the
    /// sphere without needing the segment delta.
    pub fn offset(&self, start: [u32; 3]) -> Result<SphereOffset, Bail> {
        self.offset_with(&mut Fp::new(), start)
    }

    /// [`Sphere::offset`] in the caller's `fp` ([`Fp::tracked`] gathers
    /// the guest's flags).
    pub fn offset_with(&self, fp: &mut Fp, start: [u32; 3]) -> Result<SphereOffset, Bail> {
        let relative = std::array::from_fn(|i| fp.sub(start[i], self.center[i]));
        let (_, distance_squared) = dot(fp, relative, relative);
        let radius_squared = fp.mul(self.radius, self.radius);
        let gap = fp.sub(distance_squared, radius_squared);
        fp.check(gap);
        if !fp.exact() {
            return Err(Bail::Float);
        }
        Ok(SphereOffset {
            relative,
            radius_squared,
            gap,
        })
    }
}

impl SphereOffset {
    pub fn contains_start(&self) -> bool {
        det::compare(self.gap, 0) == Ordering::Less
    }

    /// Test a segment starting on or outside the sphere. A tangent or a
    /// first contact exactly at the endpoint fails the strict pretest.
    pub fn sweep(&self, delta: [u32; 3]) -> Result<Sweep, Bail> {
        self.sweep_with(&mut Fp::new(), delta)
    }

    /// [`SphereOffset::sweep`] in the caller's `fp`.
    pub fn sweep_with(&self, fp: &mut Fp, delta: [u32; 3]) -> Result<Sweep, Bail> {
        let (x_product, projection) = dot(fp, delta, self.relative);
        fp.check(projection);
        let quadratic = if det::compare(projection, 0) == Ordering::Less {
            let (_, length_squared) = dot(fp, delta, delta);
            let projection_squared = fp.mul(projection, projection);
            let scaled_gap = fp.mul(length_squared, self.gap);
            let discriminant = fp.sub(projection_squared, scaled_gap);
            fp.check(discriminant);
            let end = if det::compare(discriminant, 0) == Ordering::Greater {
                let negative_length = fp.sub(0, length_squared);
                let gap = fp.sub(negative_length, projection);
                fp.check(gap);
                let gap_squared = if det::compare(gap, 0) == Ordering::Less {
                    None
                } else {
                    let squared = fp.mul(gap, gap);
                    fp.check(squared);
                    Some(squared)
                };
                Some(End { gap, gap_squared })
            } else {
                None
            };
            Some(Quadratic {
                length_squared,
                scaled_gap,
                discriminant,
                end,
            })
        } else {
            None
        };
        if !fp.exact() {
            return Err(Bail::Float);
        }
        Ok(Sweep {
            projection,
            x_product,
            quadratic,
        })
    }
}

impl Sweep {
    pub fn hit(&self) -> bool {
        self.quadratic.as_ref().is_some_and(|q| {
            q.end.as_ref().is_some_and(|end| {
                end.gap_squared.is_none_or(|squared| {
                    det::compare(q.discriminant, squared) == Ordering::Greater
                })
            })
        })
    }
}

// Each product rounds on its own, and the two adds associate Z, Y, X.
fn dot(fp: &mut Fp, a: [u32; 3], b: [u32; 3]) -> (u32, u32) {
    let z = fp.mul(a[2], b[2]);
    let y = fp.mul(a[1], b[1]);
    let zy = fp.add(z, y);
    let x = fp.mul(a[0], b[0]);
    (x, fp.add(zy, x))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sphere_pretest_keeps_strict_segment_boundaries() {
        let sphere = Sphere {
            center: [0; 3],
            radius: 1.0f32.to_bits(),
        };
        assert!(sphere.offset([0; 3]).unwrap().contains_start());
        for (start, delta, hit) in [
            ([2.0, 0.0, 0.0], [1.0, 0.0, 0.0], false),
            ([2.0, 0.0, 0.0], [0.0, 0.0, 0.0], false),
            ([2.0, 1.0, 0.0], [-3.0, 0.0, 0.0], false),
            ([2.0, 0.0, 0.0], [-0.5, 0.0, 0.0], false),
            ([2.0, 0.0, 0.0], [-1.0, 0.0, 0.0], false),
            ([2.0, 0.0, 0.0], [-1.5, 0.0, 0.0], true),
            ([2.0, 0.0, 0.0], [-3.0, 0.0, 0.0], true),
            ([1.0, 0.0, 0.0], [-0.5, 0.0, 0.0], true),
            ([1.0, 0.0, 0.0], [0.5, 0.0, 0.0], false),
        ] {
            let offset = sphere.offset(start.map(f32::to_bits)).unwrap();
            assert!(!offset.contains_start());
            assert_eq!(offset.sweep(delta.map(f32::to_bits)).unwrap().hit(), hit);
        }
    }

    #[test]
    fn projection_adds_z_and_y_before_x() {
        let sphere = Sphere {
            center: [0; 3],
            radius: 1.0f32.to_bits(),
        };
        let offset = sphere
            .offset([1.0, 4096.0, -4096.0].map(f32::to_bits))
            .unwrap();
        let sweep = offset
            .sweep([1.0, 4096.0, 4096.0].map(f32::to_bits))
            .unwrap();
        assert_eq!(sweep.projection, 1.0f32.to_bits());
    }

    #[test]
    fn exceptional_inputs_decline() {
        let sphere = Sphere {
            center: [0; 3],
            radius: 1.0f32.to_bits(),
        };
        assert!(matches!(sphere.offset([1, 0, 0]), Err(Bail::Float)));
        let outside = sphere.offset([2.0f32.to_bits(), 0, 0]).unwrap();
        assert!(matches!(
            outside.sweep([f32::NAN.to_bits(), 0, 0]),
            Err(Bail::Float)
        ));
    }
}
