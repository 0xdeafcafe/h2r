//! Shadow-map projections: a fitted orthographic map for the sun and a
//! six-face cube for a point light. The WGSL in `gpu.rs` mirrors
//! [`cube_face_of`] and [`ndc_to_uv`]; the tests here pin the maths down.
use crate::geom::{
    M4, V3, add, any_up, apply, box_corners, cross, look_at, mul, normalize, ortho, perspective,
    scale, sub,
};

/// A camera as the comparison renders it.
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub eye: V3,
    pub target: V3,
    pub fovy: f32,
    pub aspect: f32,
    pub near: f32,
}

impl Camera {
    pub fn view(&self) -> M4 {
        let f = normalize(sub(self.target, self.eye));
        look_at(self.eye, self.target, any_up(f))
    }
    /// The eight corners of the view frustum between distances `near` and `far`.
    pub fn frustum_corners(&self, near: f32, far: f32) -> [V3; 8] {
        let f = normalize(sub(self.target, self.eye));
        let r = normalize(cross(f, any_up(f)));
        let u = cross(r, f);
        let th = (self.fovy * 0.5).tan();
        std::array::from_fn(|i| {
            let d = if i & 4 == 0 { near } else { far };
            let sx = if i & 1 == 0 { -1.0 } else { 1.0 };
            let sy = if i & 2 == 0 { -1.0 } else { 1.0 };
            add(
                add(self.eye, scale(f, d)),
                add(scale(r, sx * d * th * self.aspect), scale(u, sy * d * th)),
            )
        })
    }
}

/// The sun's shadow map: an orthographic projection along `dir` whose x/y
/// cover `receivers` (the visible slice of the scene) and whose depth range
/// covers `casters` as well, so a caster outside the view still lands in the
/// map. Returns the view-projection and the world size of one texel.
pub fn sun_matrix(dir: V3, receivers: &[V3], casters: (V3, V3), size: u32) -> (M4, f32) {
    let dir = normalize(dir);
    let centre = receivers.iter().fold([0.0; 3], |a, &p| {
        add(a, scale(p, 1.0 / receivers.len().max(1) as f32))
    });
    let view = look_at(sub(centre, dir), centre, any_up(dir));
    let to_view = |p: V3| {
        let v = apply(&view, [p[0], p[1], p[2], 1.0]);
        [v[0], v[1], v[2]]
    };
    let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    let mut grow = |p: V3, xy: bool| {
        let v = to_view(p);
        for k in 0..3 {
            if xy || k == 2 {
                lo[k] = lo[k].min(v[k]);
                hi[k] = hi[k].max(v[k]);
            }
        }
    };
    for &p in receivers {
        grow(p, true);
    }
    for p in box_corners(casters) {
        grow(p, false);
    }
    // Square texels: pad the shorter side.
    let span = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1e-3);
    let (cx, cy) = (0.5 * (lo[0] + hi[0]), 0.5 * (lo[1] + hi[1]));
    let half = 0.5 * span;
    // View space looks down -z: near is -max z, far is -min z.
    let pad = 1e-3 * (hi[2] - lo[2]).max(1.0);
    let proj = ortho(
        cx - half,
        cx + half,
        cy - half,
        cy + half,
        -hi[2] - pad,
        -lo[2] + pad,
    );
    (mul(proj, view), span / size as f32)
}

/// Cube face `i`: forward and up, in the order +x, -x, +y, -y, +z, -z.
pub fn cube_face(i: usize) -> (V3, V3) {
    let f: [V3; 6] = [
        [1.0, 0.0, 0.0],
        [-1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, -1.0],
    ];
    (f[i], any_up(f[i]))
}

#[cfg(test)]
/// The face a direction from the light falls on: its major axis, ties going
/// to x, then y. The shader uses the same rule.
pub fn cube_face_of(d: V3) -> usize {
    let a = d.map(f32::abs);
    if a[0] >= a[1] && a[0] >= a[2] {
        if d[0] >= 0.0 { 0 } else { 1 }
    } else if a[1] >= a[2] {
        if d[1] >= 0.0 { 2 } else { 3 }
    } else if d[2] >= 0.0 {
        4
    } else {
        5
    }
}

/// View-projections of the six 90-degree faces around `pos`.
pub fn cube_matrices(pos: V3, near: f32, far: f32) -> [M4; 6] {
    let proj = perspective(std::f32::consts::FRAC_PI_2, 1.0, near, far);
    std::array::from_fn(|i| {
        let (f, up) = cube_face(i);
        mul(proj, look_at(pos, add(pos, f), up))
    })
}

#[cfg(test)]
/// NDC x/y to texture coordinates (v grows downwards).
pub fn ndc_to_uv(ndc: V3) -> [f32; 2] {
    [ndc[0] * 0.5 + 0.5, 0.5 - ndc[1] * 0.5]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{INFINITE_EPSILON, dot, perspective_infinite, project};

    fn inside(ndc: V3) -> bool {
        (-1.0001..=1.0001).contains(&ndc[0])
            && (-1.0001..=1.0001).contains(&ndc[1])
            && (-0.0001..=1.0001).contains(&ndc[2])
    }

    #[test]
    fn look_at_puts_the_target_down_negative_z() {
        let v = look_at([1.0, 2.0, 3.0], [1.0, 2.0, -7.0], [0.0, 1.0, 0.0]);
        let e = apply(&v, [1.0, 2.0, 3.0, 1.0]);
        let t = apply(&v, [1.0, 2.0, -7.0, 1.0]);
        assert!(e[..3].iter().all(|c| c.abs() < 1e-5));
        assert!((t[2] + 10.0).abs() < 1e-4 && t[0].abs() < 1e-5);
    }

    #[test]
    fn infinite_projection_keeps_points_at_infinity_inside_depth() {
        let cam = Camera {
            eye: [0.0; 3],
            target: [0.0, 1.0, 0.0],
            fovy: 1.2,
            aspect: 1.5,
            near: 0.1,
        };
        let vp = mul(
            perspective_infinite(cam.fovy, cam.aspect, cam.near),
            cam.view(),
        );
        for d in [[0.0, 1.0, 0.0], [0.3, 1.0, -0.2], [-0.4, 2.0, 0.5]] {
            let c = apply(&vp, [d[0], d[1], d[2], 0.0]);
            let z = c[2] / c[3];
            assert!((z - (1.0 - INFINITE_EPSILON)).abs() < 1e-6, "{z}");
        }
        let near = project(&vp, [0.0, 0.1, 0.0]);
        assert!(near[2].abs() < 1e-3);
        let mid = project(&vp, [0.0, 50.0, 0.0])[2];
        assert!(mid > 0.99 && mid < 1.0 - INFINITE_EPSILON);
        // Behind the camera, w is negative: clipped, never drawn.
        assert!(apply(&vp, [0.0, -1.0, 0.0, 0.0])[3] < 0.0);
    }

    #[test]
    fn sun_map_covers_receivers_and_offscreen_casters() {
        let cam = Camera {
            eye: [-10.0, -10.0, 8.0],
            target: [0.0; 3],
            fovy: 1.0,
            aspect: 4.0 / 3.0,
            near: 0.1,
        };
        let receivers = cam.frustum_corners(cam.near, 30.0);
        // A caster box well above and behind the camera.
        let casters = ([-40.0, -40.0, 0.0], [5.0, 5.0, 60.0]);
        for dir in [[0.3, 0.2, -1.0], [0.0, 0.0, -1.0], [1.0, 0.0, -0.1]] {
            let (m, texel) = sun_matrix(dir, &receivers, casters, 2048);
            for p in receivers {
                assert!(inside(project(&m, p)), "{dir:?} {p:?}");
            }
            for p in box_corners(casters) {
                let z = project(&m, p)[2];
                assert!((-1e-4..=1.0001).contains(&z), "{dir:?} caster depth {z}");
            }
            // Depth grows along the light's direction.
            let a = project(&m, [0.0; 3])[2];
            let b = project(&m, scale(normalize(dir), 1.0))[2];
            assert!(b > a);
            assert!(texel > 0.0 && texel < 1.0);
        }
    }

    #[test]
    fn sun_texel_size_halves_when_the_map_doubles() {
        let r = [
            [0.0; 3],
            [10.0, 0.0, 0.0],
            [0.0, 10.0, 0.0],
            [10.0, 10.0, 0.0],
        ];
        let b = ([0.0; 3], [10.0, 10.0, 1.0]);
        let (_, t1) = sun_matrix([0.0, 0.0, -1.0], &r, b, 1024);
        let (_, t2) = sun_matrix([0.0, 0.0, -1.0], &r, b, 2048);
        assert!((t1 / t2 - 2.0).abs() < 1e-4);
        assert!((t1 - 10.0 / 1024.0).abs() < 1e-4);
    }

    #[test]
    fn cube_faces_cover_every_direction_once() {
        let pos = [1.0, -2.0, 3.0];
        let m = cube_matrices(pos, 0.05, 100.0);
        let mut seen = [0usize; 6];
        // A deterministic spread of directions, including axis ties.
        let mut dirs: Vec<V3> = vec![[1.0, 1.0, 0.0], [0.0, -1.0, -1.0], [1.0, 1.0, 1.0]];
        for i in 0..400 {
            let a = i as f32 * 2.399_963; // golden angle
            let z = 1.0 - 2.0 * (i as f32 + 0.5) / 400.0;
            let r = (1.0 - z * z).sqrt();
            dirs.push([r * a.cos(), r * a.sin(), z]);
        }
        for d in dirs {
            let face = cube_face_of(d);
            seen[face] += 1;
            let p = add(pos, scale(normalize(d), 7.0));
            let ndc = project(&m[face], p);
            assert!(inside(ndc), "{d:?} on face {face}: {ndc:?}");
            // The face's forward axis is the direction's major axis.
            assert!(dot(cube_face(face).0, d) > 0.0);
        }
        assert!(seen.iter().all(|&n| n > 0), "{seen:?}");
    }

    #[test]
    fn uv_mapping_matches_texture_space() {
        assert_eq!(ndc_to_uv([-1.0, 1.0, 0.0]), [0.0, 0.0]);
        assert_eq!(ndc_to_uv([1.0, -1.0, 0.0]), [1.0, 1.0]);
        assert_eq!(ndc_to_uv([0.0, 0.0, 0.5]), [0.5, 0.5]);
    }

    #[test]
    fn frustum_corners_project_to_the_screen_corners() {
        let cam = Camera {
            eye: [2.0, 3.0, 4.0],
            target: [0.0; 3],
            fovy: 1.1,
            aspect: 1.6,
            near: 0.2,
        };
        let vp = mul(perspective(cam.fovy, cam.aspect, 0.2, 50.0), cam.view());
        for (i, p) in cam.frustum_corners(1.0, 20.0).into_iter().enumerate() {
            let n = project(&vp, p);
            let sx = if i & 1 == 0 { -1.0 } else { 1.0 };
            let sy = if i & 2 == 0 { -1.0 } else { 1.0 };
            assert!(
                (n[0] - sx).abs() < 1e-3 && (n[1] - sy).abs() < 1e-3,
                "{n:?}"
            );
        }
    }
}
