//! Portal projection at the mode-0 walk (`0x00173520`). The walk works in
//! tangent space. The frustum already contains the world-to-view transform
//! and the polygon that bounds the view, including off-centre projections.

use super::Decline;
use super::flood::{Facing, MAX_CORNERS, Point, Projected, Rect, Tuning};
use super::graph::Portal;
use super::guest::{self, FRUSTUM_NEGATIVE_UNIT};
use super::read::{f32_at, ptr_at, u8_at, u32_at};
use super::sphere::Rows;
use crate::GuestRead;

// The image supplies the clipping tolerance at the call to 0x0011fc80,
// the foot-in-polygon tolerance at 0x0023a220, and the initial bounds.
const NEAR_TOLERANCE_OPERAND: u32 = 0x0017_35a9;
const EDGE_TOLERANCE_OPERAND: u32 = 0x0017_3acc;
const INITIAL_DEPTH: u32 = 0x0044_aed4;
const INITIAL_RECT: u32 = 0x0046_87dc;

#[derive(Clone, Debug)]
pub struct Projection {
    basis: [[f32; 3]; 3],
    translation: [f32; 3],
    eye: [f32; 3],
    far: Option<f32>,
    negative_unit: f32,
    near_tolerance: f32,
    edge_tolerance: f32,
    initial_depth: f32,
    pub(super) initial_rect: Rect,
    rows: Rows,
    pub rect: Rect,
    pub polygon: Vec<Point>,
}

/// Cache-pool use is observable at the walk's capacity limit. A portal
/// culled before projection keeps its full reservation. One culled by the
/// far plane keeps only the corners it projected.
#[derive(Debug)]
pub struct ProjectionResult {
    pub projected: Option<(Projected, f32)>,
    pub corners_used: usize,
}

fn floats<const N: usize>(mem: &dyn GuestRead, base: u32) -> Result<[f32; N], Decline> {
    let mut values = [0.0; N];
    for (i, value) in values.iter_mut().enumerate() {
        let addr = base.checked_add(4 * i as u32).ok_or(Decline::Layout)?;
        *value = f32_at(mem, addr).ok_or(Decline::Unread)?;
        if !value.is_finite() {
            return Err(Decline::Frusta);
        }
    }
    Ok(values)
}

impl Projection {
    /// `frustum` points to the individual 0x1bc-byte input record passed
    /// to the walk. The caller checks the guest FP modes before admission.
    pub fn read(mem: &dyn GuestRead, frustum: u32) -> Result<Self, Decline> {
        let field = |offset| frustum.checked_add(offset).ok_or(Decline::Layout);
        let [scale] = floats(mem, field(4)?)?;
        let mut basis: [[f32; 3]; 3] = [
            floats(mem, field(8)?)?,
            floats(mem, field(0x14)?)?,
            floats(mem, field(0x20)?)?,
        ];
        for v in basis.iter_mut().flatten() {
            *v *= scale;
            if !v.is_finite() {
                return Err(Decline::Frusta);
            }
        }
        let count = u32_at(mem, field(0x198)?).ok_or(Decline::Unread)?;
        // The inline polygon occupies the last 32 bytes of the record.
        if !(3..=4).contains(&count) {
            return Err(Decline::Frusta);
        }
        let polygon = (0..count)
            .map(|i| floats(mem, field(0x19c + 8 * i)?))
            .collect::<Result<Vec<_>, _>>()?;
        let far = match u8_at(mem, field(0x84)?).ok_or(Decline::Unread)? {
            0 => None,
            _ => Some(floats::<1>(mem, field(0x88)?)?[0]),
        };
        let initial_rect = ptr_at(mem, INITIAL_RECT).ok_or(Decline::Unread)?;
        let [near_tolerance] = floats(mem, NEAR_TOLERANCE_OPERAND)?;
        let [edge_tolerance] = floats(mem, EDGE_TOLERANCE_OPERAND)?;
        let [negative_unit] = floats(mem, FRUSTUM_NEGATIVE_UNIT)?;
        if near_tolerance <= 0.0 || edge_tolerance < 0.0 || negative_unit >= 0.0 {
            return Err(Decline::Frusta);
        }
        Ok(Self {
            basis,
            translation: floats(mem, field(0x2c)?)?,
            eye: floats(mem, field(0x60)?)?,
            rect: Rect(floats(mem, field(0xa0)?)?),
            polygon,
            far,
            near_tolerance,
            edge_tolerance,
            negative_unit,
            rows: guest::projection_rows(mem)?,
            initial_rect: Rect(floats(mem, initial_rect)?),
            initial_depth: floats::<1>(mem, INITIAL_DEPTH)?[0],
        })
    }

    /// `0x001429d0`: the scaled basis is multiplied by the world point
    /// before adding the paired sums. Subtracting the eye first changes
    /// rounding when the transform has a translation.
    fn transform(&self, [x, y, z]: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|i| {
            (x * self.basis[0][i] + y * self.basis[1][i])
                + (z * self.basis[2][i] + self.translation[i])
        })
    }

    /// Single-plane crossing from 0x0011fc80. At a crossing the guest
    /// interpolates *all three* coordinates, including z. Retaining the
    /// interpolated z matters for the subsequent reciprocal and depth.
    fn clip_near(&self, ring: &[[f32; 3]]) -> Result<Vec<[f32; 3]>, Decline> {
        let mut clipped = Vec::with_capacity(ring.len());
        let Some(&last) = ring.last() else {
            return Ok(clipped);
        };
        let side = |p: [f32; 3]| self.negative_unit * p[2] - self.near_tolerance;
        let mut previous = self.transform(last);
        let mut before = side(previous);
        for &world in ring {
            let current = self.transform(world);
            let next = side(current);
            if !current.iter().all(|v| v.is_finite()) || !next.is_finite() {
                return Err(Decline::Frusta);
            }
            if (before >= 0.0) != (next >= 0.0) {
                let t = before / (before - next);
                let intersection: [f32; 3] =
                    std::array::from_fn(|k| (current[k] - previous[k]) * t + previous[k]);
                if !intersection.iter().all(|v| v.is_finite()) {
                    return Err(Decline::Frusta);
                }
                clipped.push(intersection);
            }
            if next >= 0.0 {
                clipped.push(current);
            }
            if clipped.len() > MAX_CORNERS {
                return Err(Decline::Overflow);
            }
            previous = current;
            before = next;
        }
        Ok(clipped)
    }

    /// `0x00173910`: project the foot of the eye to two axes and test
    /// the directed edge normals with the guest's tolerance.
    fn inside(&self, portal: &Portal) -> Result<bool, Decline> {
        let [nx, ny, nz, d] = portal.plane;
        let n = [nx, ny, nz];
        let [x, y, z] = self.eye;
        let back = 0.0 - (((z * nz + y * ny) + x * nx) - d);
        let [a, b, c] = n.map(f32::abs);
        let axis = if c >= b && c >= a {
            2
        } else if b >= a {
            1
        } else {
            0
        };
        let [u, v] = self.rows[2 * axis + usize::from(n[axis] > 0.0)].map(usize::from);
        let foot = |k: usize| n[k] * back + self.eye[k];
        let [pu, pv] = [foot(u), foot(v)];
        for (i, p) in portal.vertices.iter().enumerate() {
            let q = portal.vertices[(i + 1) % portal.vertices.len()];
            let side = (pv - p[v]) * (q[u] - p[u]) - (pu - p[u]) * (q[v] - p[v]);
            if !side.is_finite() {
                return Err(Decline::Frusta);
            }
            if side < -self.edge_tolerance {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn project(&self, portal: &Portal, tuning: &Tuning) -> Result<ProjectionResult, Decline> {
        if portal.vertices.len() > MAX_CORNERS || !portal.plane.iter().all(|x| x.is_finite()) {
            return Err(Decline::Layout);
        }
        let clipped = self.clip_near(&portal.vertices)?;
        let [x, y, z] = self.eye.map(f64::from);
        let [nx, ny, nz, d] = portal.plane.map(f64::from);
        let distance = ((z * nz + y * ny) + x * nx) - d;
        let near_plane = distance.abs() <= tuning.straddle;
        let inside = near_plane && self.inside(portal)?;
        if clipped.is_empty() && !inside {
            return Ok(ProjectionResult {
                projected: None,
                corners_used: MAX_CORNERS,
            });
        }
        let facing = if near_plane || clipped.len() < 3 {
            Facing::Straddle
        } else if (distance as f32) < 0.0 {
            Facing::Back
        } else {
            Facing::Front
        };
        let mut nearest = self.initial_depth;
        let mut rect = self.initial_rect;
        let mut polygon = Vec::with_capacity(clipped.len());
        for [x, y, z] in clipped {
            let reciprocal = self.negative_unit / z;
            let [u, v] = [x * reciprocal, y * reciprocal];
            if !u.is_finite() || !v.is_finite() {
                return Err(Decline::Frusta);
            }
            polygon.push([u, v]);
            // Bounds and depth visit the source vertices in forward order
            // even on a backwards-facing portal. Keep the old bits on ties.
            if u < rect.0[0] {
                rect.0[0] = u;
            }
            if u > rect.0[1] {
                rect.0[1] = u;
            }
            if v < rect.0[2] {
                rect.0[2] = v;
            }
            if v > rect.0[3] {
                rect.0[3] = v;
            }
            let depth = 0.0 - z;
            if depth < nearest {
                nearest = depth;
            }
        }
        match facing {
            Facing::Back => polygon.reverse(),
            Facing::Straddle if inside => {
                rect = self.rect;
                polygon.clone_from(&self.polygon);
                if f64::from(nearest) > tuning.straddle {
                    nearest = tuning.straddle as f32;
                }
            }
            Facing::Straddle => {
                let [x0, x1, y0, y1] = rect.0;
                let w = tuning.widen;
                rect = Rect([x0 - w, x1 + w, y0 - w, y1 + w]);
                if !rect.0.iter().all(|v| v.is_finite()) {
                    return Err(Decline::Frusta);
                }
                polygon = rect.corners();
            }
            Facing::Front => {}
        }
        let corners_used = polygon.len();
        let projected = if self.far.is_some_and(|far| nearest > far) {
            None
        } else {
            Some((
                Projected {
                    facing,
                    rect,
                    polygon,
                },
                nearest,
            ))
        };
        Ok(ProjectionResult {
            projected,
            corners_used,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Mem;
    use crate::visibility::flood::tests::TUNING;

    // A translated, scaled world-to-view matrix. The frustum's tangent
    // polygon has asymmetric limits and a deliberately non-default start.
    const FRUSTUM: u32 = 0x1200;
    const ROOT: [Point; 4] = [[2.0, 1.0], [-3.0, 1.0], [-3.0, -1.0], [2.0, -1.0]];

    fn view() -> Projection {
        let mut m = Mem::default();
        fn values(m: &mut Mem, base: u32, row: &[f32]) {
            for (i, &v) in row.iter().enumerate() {
                m.f(base + 4 * i as u32, v);
            }
        }
        // Byte operands need not be word-aligned.
        let mut operand = |addr: u32, value: f32| {
            for (i, byte) in value.to_le_bytes().into_iter().enumerate() {
                let p = addr + i as u32;
                let shift = (p % 4) * 8;
                let word = m.0.entry(p & !3).or_default();
                *word = (*word & !(0xff << shift)) | (u32::from(byte) << shift);
            }
        };
        operand(NEAR_TOLERANCE_OPERAND, 0.5);
        operand(EDGE_TOLERANCE_OPERAND, 0.03125);
        m.f(FRUSTUM_NEGATIVE_UNIT, -1.0);
        m.f(INITIAL_DEPTH, 1024.0);
        m.w(INITIAL_RECT, 0x1c00);
        values(&mut m, 0x1c00, &[128.0, -128.0, 128.0, -128.0]);
        for i in 0..6 {
            // Read each row independently of 32-bit alignment.
            for (j, axis) in [0u16, 1, 2].into_iter().enumerate() {
                let addr = guest::PROJECTION_ROWS + 6 * i + j as u32 * 2;
                *m.0.entry(addr & !3).or_default() |= u32::from(axis) << ((addr % 4) * 8);
            }
        }
        m.f(FRUSTUM + 4, 2.0);
        values(
            &mut m,
            FRUSTUM + 8,
            &[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        );
        values(&mut m, FRUSTUM + 0x2c, &[-2.0, 0.0, 0.0]);
        values(&mut m, FRUSTUM + 0x60, &[1.0, 0.0, 0.0]);
        m.w(FRUSTUM + 0x84, 0);
        values(&mut m, FRUSTUM + 0xa0, &[-3.0, 2.0, -1.0, 1.0]);
        m.w(FRUSTUM + 0x198, 4);
        for (i, point) in ROOT.iter().enumerate() {
            values(&mut m, FRUSTUM + 0x19c + i as u32 * 8, point);
        }
        Projection::read(&m, FRUSTUM).unwrap()
    }

    fn portal(plane: [f32; 4]) -> Portal {
        Portal {
            clusters: [2, 4],
            plane,
            flags: 0,
            sphere: [0.0; 4],
            vertices: vec![[0.0, -1.0, -2.0], [2.0, -1.0, -2.0], [1.0, 2.0, -2.0]],
        }
    }

    #[test]
    fn tangent_projection_uses_the_matrix_scale_and_reverses_only_the_back_face() {
        let view = view();
        let front = portal([0.0, 0.0, 1.0, -2.0]);
        let mut back = front.clone();
        back.plane = [0.0, 0.0, -1.0, 2.0];
        let (projected, depth) = view.project(&front, &TUNING).unwrap().projected.unwrap();
        assert_eq!(depth, 4.0);
        assert_eq!(projected.facing, Facing::Front);
        assert_eq!(projected.rect, Rect([-0.5, 0.5, -0.5, 1.0]));
        assert_eq!(projected.polygon, [[-0.5, -0.5], [0.5, -0.5], [0.0, 1.0]]);
        let (reverse, depth) = view.project(&back, &TUNING).unwrap().projected.unwrap();
        assert_eq!(depth, 4.0);
        assert_eq!(reverse.facing, Facing::Back);
        assert_eq!(reverse.rect, projected.rect);
        assert_eq!(
            reverse.polygon,
            projected.polygon.into_iter().rev().collect::<Vec<_>>()
        );
    }

    #[test]
    fn crossing_the_near_plane_emits_intersections_in_ring_order() {
        let view = view();
        let mut p = portal([0.0, 0.0, 1.0, -1.0]);
        p.vertices = vec![[1.0, 0.0, 0.0], [2.0, 0.0, -0.5], [1.0, 1.0, -0.5]];
        let result = view.project(&p, &TUNING).unwrap();
        let (projected, depth) = result.projected.unwrap();
        assert_eq!(result.corners_used, 4);
        assert_eq!(depth, 0.5);
        assert_eq!(
            projected.polygon,
            [[0.0, 2.0], [2.0, 0.0], [2.0, 0.0], [0.0, 2.0]]
        );
    }

    #[test]
    fn projection_cache_distinguishes_facing_and_far_plane_culls() {
        let mut view = view();
        let mut p = portal([0.0, 0.0, 1.0, -2.0]);
        view.far = Some(3.0);
        let outside_far = view.project(&p, &TUNING).unwrap();
        assert!(outside_far.projected.is_none());
        assert_eq!(outside_far.corners_used, p.vertices.len());
        for v in &mut p.vertices {
            v[2] = 2.0;
        }
        let behind = view.project(&p, &TUNING).unwrap();
        assert!(behind.projected.is_none());
        assert_eq!(behind.corners_used, MAX_CORNERS);
    }

    #[test]
    fn a_straddle_containing_the_eye_copies_the_actual_root_even_with_no_near_vertices() {
        let view = view();
        let mut p = portal([0.0, 0.0, 1.0, 0.0]);
        for v in &mut p.vertices {
            v[2] = 0.0;
        }
        let (projected, depth) = view.project(&p, &TUNING).unwrap().projected.unwrap();
        assert_eq!(projected.polygon, ROOT);
        assert_eq!(projected.rect, view.rect);
        assert_eq!(depth, TUNING.straddle as f32);
        assert_eq!(projected.facing, Facing::Straddle);
        // Reversing the winding changes the directed half-space tests.
        p.vertices.reverse();
        assert!(view.project(&p, &TUNING).unwrap().projected.is_none());
    }
}
