//! Tangent-space window clipping at the portal walk's intersection call
//! (0x00173380). The portal polygon is the subject. Each directed edge
//! of the parent window clips it to its left half-plane, starting with
//! the edge from the last parent vertex to the first.

use super::Decline;
use super::flood::{MAX_CORNERS, Point};
use super::guest::{FRUSTUM_UNIT, NORMALIZE_EPSILON};
use super::read::f32_at;
use crate::GuestRead;

/// The immediate passed to the polygon clipper and the lower threshold
/// used in the segment/plane intersection.
const CLIP_TOLERANCE: u32 = 0x0017_338a;
const NEGATIVE_EPSILON: u32 = 0x0045_ddfc;

pub struct Inputs {
    tolerance: f32,
    epsilon: f32,
    negative_epsilon: f32,
    unit: f32,
}

impl Inputs {
    pub fn read(mem: &dyn GuestRead) -> Result<Self, Decline> {
        let read = |addr| f32_at(mem, addr).ok_or(Decline::Unread).and_then(finite);
        let inputs = Self {
            tolerance: read(CLIP_TOLERANCE)?,
            epsilon: read(NORMALIZE_EPSILON)?,
            negative_epsilon: read(NEGATIVE_EPSILON)?,
            unit: read(FRUSTUM_UNIT)?,
        };
        if inputs.tolerance < 0.0 || inputs.epsilon < 0.0 || inputs.negative_epsilon > 0.0 {
            return Err(Decline::Layout);
        }
        Ok(inputs)
    }

    /// A result with fewer than three vertices does not make a portal
    /// window. The caller supplies the guest's vertex winding and the
    /// round-to-nearest, double-precision x87 environment.
    pub fn clip(&self, portal: &[Point], parent: &[Point]) -> Result<Vec<Point>, Decline> {
        if portal.len() > MAX_CORNERS || parent.len() > MAX_CORNERS {
            return Err(Decline::Overflow);
        }
        if portal
            .iter()
            .chain(parent)
            .flatten()
            .any(|v| !v.is_finite())
        {
            return Err(Decline::Frusta);
        }
        if parent.len() < 3 {
            return Err(Decline::Frusta);
        }
        let mut subject = portal.to_vec();
        let mut clipped = Vec::with_capacity(MAX_CORNERS);
        let mut previous = parent[parent.len() - 1];
        for &current in parent {
            if subject.is_empty() {
                break;
            }
            if let Some(plane) = self.plane(previous, current)? {
                clipped.clear();
                self.clip_plane(plane, &subject, &mut clipped)?;
                std::mem::swap(&mut subject, &mut clipped);
            }
            previous = current;
        }
        Ok(subject)
    }

    fn plane(&self, a: Point, b: Point) -> Result<Option<Plane>, Decline> {
        // 0x0023a320 stores the first component to a float. The other
        // subtraction remains on the x87 stack across the store and is
        // multiplied by its rounded copy. The epsilon test precedes the
        // rounding of the length used for the reciprocal.
        let nx = finite((f64::from(a[1]) - f64::from(b[1])) as f32)?;
        let ny_wide = f64::from(b[0]) - f64::from(a[0]);
        let ny = finite(ny_wide as f32)?;
        let length = (ny_wide * f64::from(ny) + f64::from(nx) * f64::from(nx)).sqrt();
        if length < f64::from(self.epsilon) || length as f32 == 0.0 {
            return Ok(None);
        }
        let scale = finite(self.unit / finite(length as f32)?)?;
        let normal = [finite(nx * scale)?, finite(ny * scale)?];
        Ok(Some(Plane {
            normal,
            offset: finite(b[1] * normal[1] + b[0] * normal[0])?,
        }))
    }

    fn clip_plane(
        &self,
        plane: Plane,
        points: &[Point],
        output: &mut Vec<Point>,
    ) -> Result<(), Decline> {
        let mut sides = [Side::On; MAX_CORNERS];
        let mut inside = false;
        let mut outside = false;
        for (&p, side) in points.iter().zip(&mut sides) {
            let d = plane.distance(p)?;
            if d > self.tolerance {
                *side = Side::Inside;
                inside = true;
            } else if d < -self.tolerance {
                *side = Side::Outside;
                outside = true;
            }
        }
        if !outside {
            output.extend_from_slice(points);
            return Ok(());
        }
        if !inside {
            return Ok(());
        }
        let mut previous = points.len() - 1;
        for (i, &current) in points.iter().enumerate() {
            let crossing = match (sides[previous], sides[i]) {
                (Side::Inside, Side::Outside) => Some((current, points[previous])),
                (Side::Outside, Side::Inside) => Some((points[previous], current)),
                _ => None,
            };
            if let Some((out, into)) = crossing {
                // Both directions interpolate from the outside vertex.
                let delta = [out[0] - into[0], out[1] - into[1]];
                let denom = finite(plane.normal[0] * delta[0] + plane.normal[1] * delta[1])?;
                if denom < self.negative_epsilon || denom > self.epsilon {
                    let t = finite(plane.distance(out)? / denom)?;
                    output.push([
                        finite(out[0] - delta[0] * t)?,
                        finite(out[1] - delta[1] * t)?,
                    ]);
                }
            }
            if sides[i] != Side::Outside {
                output.push(current);
            }
            if output.len() > MAX_CORNERS {
                // The guest truncates this edge's output before the next
                // edge. Leave this exceptional case to that path.
                return Err(Decline::Overflow);
            }
            previous = i;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Inside,
    On,
    Outside,
}

#[derive(Clone, Copy)]
struct Plane {
    normal: Point,
    offset: f32,
}

impl Plane {
    fn distance(self, [x, y]: Point) -> Result<f32, Decline> {
        finite((y * self.normal[1] + x * self.normal[0]) - self.offset)
    }
}

fn finite(v: f32) -> Result<f32, Decline> {
    v.is_finite().then_some(v).ok_or(Decline::Frusta)
}

#[cfg(test)]
mod tests {
    use super::*;

    const INPUTS: Inputs = Inputs {
        tolerance: 0.125,
        epsilon: 0.03125,
        negative_epsilon: -0.03125,
        unit: 1.0,
    };

    const WINDOW: [Point; 4] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];

    #[test]
    fn replaces_both_crossings_in_portal_vertex_order() {
        let portal = [[-2.0, -0.5], [0.5, -0.5], [0.5, 0.5], [-2.0, 0.5]];
        assert_eq!(
            INPUTS.clip(&portal, &WINDOW).unwrap(),
            [[-1.0, -0.5], [0.5, -0.5], [0.5, 0.5], [-1.0, 0.5]]
        );
    }

    #[test]
    fn keeps_tolerance_band_when_no_point_is_outside() {
        let portal = [[-1.125, -0.5], [-1.0625, -0.5], [-1.0625, 0.5]];
        assert_eq!(INPUTS.clip(&portal, &WINDOW).unwrap(), portal);
        let contact = [[-1.0, -0.5], [-1.0, 0.5], [-2.0, 0.0]];
        assert!(INPUTS.clip(&contact, &WINDOW).unwrap().is_empty());
    }

    #[test]
    fn a_degenerate_parent_edge_preserves_the_subject_for_the_next_edge() {
        let window = [WINDOW[0], WINDOW[0], WINDOW[1], WINDOW[2], WINDOW[3]];
        let subject = [[-0.0, -0.5], [-0.25, 0.0], [-0.5, 0.5]];
        let out = INPUTS.clip(&subject, &window).unwrap();
        assert_eq!(out, subject);
        assert_eq!(out[0][0].to_bits(), (-0.0f32).to_bits());
    }

    #[test]
    fn oversized_and_nonfinite_windows_decline() {
        assert_eq!(
            INPUTS.clip(&[[0.0; 2]; MAX_CORNERS + 1], &WINDOW),
            Err(Decline::Overflow)
        );
        assert_eq!(
            INPUTS.clip(&[[f32::NAN; 2]; 3], &WINDOW),
            Err(Decline::Frusta)
        );
        let mut portal = [[0.0; 2]; MAX_CORNERS];
        portal[MAX_CORNERS / 2] = [-3.0, 0.0];
        assert_eq!(INPUTS.clip(&portal, &WINDOW), Err(Decline::Overflow));
    }
}
