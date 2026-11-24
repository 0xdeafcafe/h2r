//! The window frusta of mode 0 and the object test against them
//! (docs/graphics/native-visibility.md, "Objects").
//!
//! For each window left after the merge, `0x00172ee0` builds a frustum with
//! `0x00163db0` from the window's rectangle, not its polygon: a box and six
//! planes, all in world space. The per-object test `0x00133050` calls
//! `0x00132e60`, which in mode 0 calls `0x00165010`: an object is seen when
//! its sphere reaches any frustum of its region (`0x00181800`).
//!
//! The view's local frame here is the lens's: x right, y up, z back (the
//! eye looks down −z), the matrix's scale one. A window's rectangle is in
//! tangent units (`x / depth`), the flood's screen units through
//! [`Lens::tangent`]. Every value the guest's code reads from its data is
//! read from the running image ([`Inputs`], `super::guest::frustum_inputs`).

use super::flood::{Lens, Rect};

/// What the build reads besides the window and the lens: values from the
/// image's data and the view frustum's clip plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Inputs {
    /// The right and top planes' outward term, the eye plane's normal and
    /// the normalise's numerator (`0x0045dbc0`).
    pub unit: f32,
    /// The left and bottom planes' outward term and the far plane's normal
    /// (`0x0045dbcc`).
    pub negative_unit: f32,
    /// A normal shorter than this is left as it is (`0x0045dbdc`, read by
    /// `0x00030bf0`).
    pub epsilon: f32,
    /// The box the frustum's bounds start from (min x, max x, min y, max y,
    /// min z, max z; the six floats `[0x004687e0]` points at).
    pub bounds: [f32; 6],
    /// The view frustum's clip plane (`+0x74`), when its byte at `+0x6c`
    /// is set: negated, it replaces the eye plane.
    pub clip_plane: Option<[f32; 4]>,
}

/// One window's frustum (`0x00163db0`'s record, 0x108 bytes; its box at
/// `+0x60`, its planes at `+0x78`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frustum {
    /// Min x, max x, min y, max y, min z, max z: the eye and the window's
    /// four corners at the far distance.
    pub bounds: [f32; 6],
    /// Normal and distance, the outside positive: left, right, bottom, top,
    /// the eye (or the clip plane), far.
    pub planes: [[f32; 4]; 6],
}

/// The lens as the guest's view matrix: the local axes in world space and
/// the eye.
struct Basis {
    axes: [[f32; 3]; 3],
    origin: [f32; 3],
}

impl Basis {
    fn new(lens: &Lens) -> Self {
        let [right, up, forward] = lens.axes();
        Self {
            axes: [right, up, forward.map(|v| -v)],
            origin: lens.position,
        }
    }

    /// `0x001429d0`: a local point to the world.
    fn point(&self, [x, y, z]: [f32; 3]) -> [f32; 3] {
        let [r0, r1, r2] = self.axes;
        [0, 1, 2].map(|i| (x * r0[i] + y * r1[i]) + (z * r2[i] + self.origin[i]))
    }

    /// A local plane to the world, in `0x00163db0`'s order.
    fn plane(&self, [x, y, z]: [f32; 3], d: f32) -> [f32; 4] {
        let [r0, r1, r2] = self.axes;
        let n = [0, 1, 2].map(|i| (r2[i] * z + r1[i] * y) + r0[i] * x);
        let t = self.origin;
        [n[0], n[1], n[2], ((t[1] * n[1] + d) + t[2] * n[2]) + t[0] * n[0]]
    }
}

/// `0x00030bf0`: the length on the x87 stack, compared there with the
/// epsilon; the scale is the numerator over the length stored as a float.
fn normalize(v: [f32; 3], inputs: &Inputs) -> [f32; 3] {
    let [x, y, z] = v.map(f64::from);
    let length = ((x * x + y * y) + z * z).sqrt();
    if f64::from(inputs.epsilon) > length.abs() {
        return v;
    }
    let scale = inputs.unit / length as f32;
    v.map(|c| c * scale)
}

impl Frustum {
    /// `0x00163db0`: the frustum through the eye and the window `rect`
    /// (tangent units), out to the lens's far distance. `None` when the
    /// rectangle is empty (max not above min, or NaN), as the guest
    /// returns false there.
    pub fn build(rect: [f32; 4], lens: &Lens, inputs: &Inputs) -> Option<Self> {
        let [x0, x1, y0, y1] = rect;
        let wide = x1 > x0 && y1 > y0;
        if !wide {
            return None;
        }
        let basis = Basis::new(lens);
        let k = lens.far;
        let (x0k, x1k, y0k, y1k, back) = (x0 * k, x1 * k, y0 * k, y1 * k, 0.0 - k);
        let corners = [
            [x0k, y1k, back],
            [x0k, y0k, back],
            [x1k, y1k, back],
            [x1k, y0k, back],
        ]
        .map(|c| basis.point(c));
        let mut bounds = inputs.bounds;
        for p in corners.iter().chain([&lens.position]) {
            for axis in 0..3 {
                if bounds[2 * axis] > p[axis] {
                    bounds[2 * axis] = p[axis];
                }
                if p[axis] > bounds[2 * axis + 1] {
                    bounds[2 * axis + 1] = p[axis];
                }
            }
        }
        let (one, minus) = (inputs.unit, inputs.negative_unit);
        let side = |n: [f32; 3]| basis.plane(normalize(n, inputs), 0.0);
        let eye = match inputs.clip_plane {
            Some(p) => p.map(|v| 0.0 - v),
            None => basis.plane([0.0, 0.0, one], 0.0),
        };
        Some(Self {
            bounds,
            planes: [
                side([minus, 0.0, 0.0 - x0]),
                side([one, 0.0, x1]),
                side([0.0, minus, 0.0 - y0]),
                side([0.0, one, y1]),
                eye,
                basis.plane([0.0, 0.0, minus], k),
            ],
        })
    }

    /// `0x00181800`, true for its "crosses" and "inside": the sphere is
    /// outside when it is wholly past a side of the box, or farther than
    /// its radius outside a plane; NaN comparisons do not reject.
    pub fn admits(&self, centre: [f32; 3], radius: f32) -> bool {
        let [cx, cy, cz] = centre;
        let r = radius;
        let b = self.bounds;
        if cx - r > b[1] || cy - r > b[3] || cz - r > b[5] {
            return false;
        }
        if b[0] > cx + r || b[2] > r + cy || b[4] > r + cz {
            return false;
        }
        !self
            .planes
            .iter()
            .any(|&[nx, ny, nz, d]| ((nz * cz + ny * cy) + nx * cx) - d > r)
    }
}

/// The frusta of a view's windows, in the windows' order, each with its
/// cluster; a window with an empty rectangle has none.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowFrusta(pub Vec<(u16, Frustum)>);

impl WindowFrusta {
    /// `0x00172ee0`'s builds over the merged windows.
    pub fn build(windows: &[(u16, Rect)], lens: &Lens, inputs: &Inputs) -> Self {
        Self(
            windows
                .iter()
                .filter_map(|&(cluster, Rect(r))| {
                    let [x0, y0] = lens.tangent([r[0], r[2]]);
                    let [x1, y1] = lens.tangent([r[1], r[3]]);
                    Frustum::build([x0, x1, y0, y1], lens, inputs).map(|f| (cluster, f))
                })
                .collect(),
        )
    }

    /// `0x00165010` for one view frustum: the sphere reaches a frustum of
    /// `cluster`'s windows.
    pub fn admits(&self, cluster: u16, centre: [f32; 3], radius: f32) -> bool {
        self.0
            .iter()
            .any(|(c, f)| *c == cluster && f.admits(centre, radius))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::visibility::flood::tests::camera_at;
    use frame_scene::RenderCamera;

    /// Invented values for the image's.
    pub(crate) const INPUTS: Inputs = Inputs {
        unit: 1.0,
        negative_unit: -1.0,
        epsilon: 1e-4,
        bounds: [f32::MAX, -f32::MAX, f32::MAX, -f32::MAX, f32::MAX, -f32::MAX],
        clip_plane: None,
    };

    /// At the origin looking down +x, z up, the projection's scales one so
    /// screen units are tangent units. Right is −y.
    fn lens() -> Lens {
        Lens::new(&RenderCamera {
            projection_xy: Some([1.0, 1.0, 0.0, 0.0]),
            ..camera_at([0.0, 0.0, 0.0], [1.0, 0.0, 0.0])
        })
        .unwrap()
    }

    fn close(a: [f32; 4], b: [f32; 4]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    /// The right half of the screen.
    fn right_half() -> Frustum {
        Frustum::build([0.0, 1.0, -1.0, 1.0], &lens(), &INPUTS).unwrap()
    }

    #[test]
    fn a_window_makes_planes_through_the_eye_and_its_edges() {
        let f = right_half();
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let expected = [
            [0.0, 1.0, 0.0, 0.0],
            [-h, -h, 0.0, 0.0],
            [-h, 0.0, -h, 0.0],
            [-h, 0.0, h, 0.0],
            [-1.0, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 1000.0],
        ];
        for (i, (got, want)) in f.planes.iter().zip(expected).enumerate() {
            assert!(close(*got, want), "plane {i}: {got:?}");
        }
        assert_eq!(
            f.bounds,
            [0.0, 1000.0, -1000.0, 0.0, -1000.0, 1000.0],
            "the eye and the far corners"
        );
        assert_eq!(
            Frustum::build([0.5, 0.5, -1.0, 1.0], &lens(), &INPUTS),
            None,
            "an empty window"
        );
    }

    #[test]
    fn a_sphere_just_inside_each_plane_is_admitted_and_just_outside_is_not() {
        let f = right_half();
        let r = 0.5;
        // A point on each plane, inside the others.
        let on = [
            [10.0, 0.0, 0.0],
            [10.0, -10.0, 0.0],
            [10.0, -5.0, -10.0],
            [10.0, -5.0, 10.0],
            [0.0, 0.0, 0.0],
            [1000.0, -5.0, 0.0],
        ];
        for (i, (p, plane)) in on.iter().zip(f.planes).enumerate() {
            let at = |by: f32| [0, 1, 2].map(|k| p[k] + plane[k] * by);
            assert!(f.admits(at(r - 0.1), r), "inside plane {i}");
            assert!(!f.admits(at(r + 0.1), r), "outside plane {i}");
        }
    }

    #[test]
    fn the_frusta_differ_from_a_wedge_between_the_planes() {
        let lens = lens();
        let screen = Frustum::build([-1.0, 1.0, -1.0, 1.0], &lens, &INPUTS).unwrap();
        // Between the eye and the near plane (0.0625): a wedge clipped at
        // the near plane refused it; the eye plane admits it.
        assert!(screen.admits([-0.05, 0.0, 0.0], 0.1));
        // Past the far corner: inside every plane, but off the box.
        let corner = [1000.9, -1001.2, 0.0];
        assert!(
            screen.planes.iter().all(|&[nx, ny, nz, d]| {
                nx * corner[0] + ny * corner[1] + nz * corner[2] - d <= 1.0
            }),
            "the planes alone admit it"
        );
        assert!(!screen.admits(corner, 1.0), "the box refuses it");
    }

    #[test]
    fn a_clip_plane_replaces_the_eye_plane_negated() {
        let inputs = Inputs {
            clip_plane: Some([1.0, 0.0, 0.0, 5.0]),
            ..INPUTS
        };
        let f = Frustum::build([-1.0, 1.0, -1.0, 1.0], &lens(), &inputs).unwrap();
        assert_eq!(f.planes[4], [-1.0, 0.0, 0.0, -5.0]);
        assert!(!f.admits([2.0, 0.0, 0.0], 0.5), "before the clip plane");
        assert!(f.admits([6.0, 0.0, 0.0], 0.5));
    }

    #[test]
    fn the_windows_of_a_cluster_admit_its_objects() {
        let lens = lens();
        let frusta = WindowFrusta::build(
            &[
                (2, Rect([0.0, 1.0, -1.0, 1.0])),
                (3, Rect([0.0, 0.0, -1.0, 1.0])),
            ],
            &lens,
            &INPUTS,
        );
        assert_eq!(frusta.0.len(), 1, "the empty window has no frustum");
        assert!(frusta.admits(2, [10.0, -3.0, 0.0], 0.5));
        assert!(!frusta.admits(2, [10.0, 3.0, 0.0], 0.5));
        assert!(!frusta.admits(3, [10.0, -3.0, 0.0], 0.5));
    }
}
