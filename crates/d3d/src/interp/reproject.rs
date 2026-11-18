//! E1, rotation-only reprojection (docs/game/high-framerate.md, section 3
//! "E1" and section 8): frame N's world is drawn once more, with overscan,
//! into a surface of its own; each display frame turns that image by the
//! camera's rotation since tick N in one full-screen warp, and the
//! first-person and HUD draws go on top as frame N drew them.
//!
//! A pure rotation needs no depth: a pixel's ray in the display camera,
//! turned into the tick camera and projected, is where to sample. In
//! normalised device coordinates that is the homography
//! `H = M_w * R_N * R_dᵀ * M_d⁻¹`, where `M` is the projection acting on
//! directions (rows 0, 1 and 3 of `K`, columns 0-2), `M_w` the overscanned
//! one the world image was drawn with, `R_N` the tick camera's rotation
//! and `R_d` the display camera's. The viewport (`SET_VIEWPORT_SCALE` and
//! `OFFSET`, pixels = ndc * scale + offset) turns it into texture
//! coordinates.

use super::camera::Persp;
use super::math::{Mat4, Quat};
use super::model::{FrameModel, Kind, Tuning, read4, write};
use crate::backend::ResolvedDraw;
use crate::shader::Constants;

/// How much wider (in the tangent of the half angle) the world image is
/// drawn than the screen, so a turn shows drawn pixels at the edges: 10%.
pub const OVERSCAN: f64 = 0.1;

/// The factor the world's clip `x` and `y` are scaled by for [`OVERSCAN`].
pub fn overscan_scale() -> f64 {
    1.0 / (1.0 + OVERSCAN)
}

/// Where normalised device coordinates land on the target: guest pixels
/// `ndc * scale + offset`, of a `size` target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Screen {
    pub scale: [f64; 2],
    pub offset: [f64; 2],
    pub size: [f64; 2],
}

impl Screen {
    /// From a world draw: its viewport registers, or D3D's default mapping
    /// over the target when they were never set.
    pub fn of(draw: &ResolvedDraw) -> Screen {
        let size = [draw.target.width as f64, draw.target.height as f64];
        let s = draw.hw.viewport_scale;
        let o = draw.hw.viewport_offset;
        if s[0] == 0.0 || s[1] == 0.0 {
            return Screen {
                scale: [size[0] / 2.0, -size[1] / 2.0],
                offset: [size[0] / 2.0, size[1] / 2.0],
                size,
            };
        }
        Screen {
            scale: [s[0] as f64, s[1] as f64],
            offset: [o[0] as f64, o[1] as f64],
            size,
        }
    }
}

type M3 = [[f64; 3]; 3];

fn mul3(a: &M3, b: &M3) -> M3 {
    let mut m = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    m
}

fn inv3(m: &M3) -> Option<M3> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-300 || !det.is_finite() {
        return None;
    }
    let c = |a: usize, b: usize, c: usize, d: usize| m[a][b] * m[c][d] - m[a][d] * m[c][b];
    Some([
        [
            c(1, 1, 2, 2) / det,
            -c(0, 1, 2, 2) / det,
            c(0, 1, 1, 2) / det,
        ],
        [
            -c(1, 0, 2, 2) / det,
            c(0, 0, 2, 2) / det,
            -c(0, 0, 1, 2) / det,
        ],
        [
            c(1, 0, 2, 1) / det,
            -c(0, 0, 2, 1) / det,
            c(0, 0, 1, 1) / det,
        ],
    ])
}

/// The projection's action on directions: rows 0, 1 and 3, columns 0-2.
fn directions(k: &Mat4) -> M3 {
    [
        [k[0][0], k[0][1], k[0][2]],
        [k[1][0], k[1][1], k[1][2]],
        [k[3][0], k[3][1], k[3][2]],
    ]
}

/// The warp, in normalised texture coordinates of the target: the display
/// texel `(u, v, 1)` samples the world image at `H (u, v, 1)` (divided by
/// the third component). `tick` is frame N's camera (the world image's,
/// before overscan), `display` the display camera's rotation, `s` the
/// overscan scale ([`overscan_scale`]).
pub fn homography(tick: &Persp, display: Quat, s: f64, screen: &Screen) -> Option<[[f32; 3]; 3]> {
    let m_d = directions(&tick.projection);
    let m_w = mul3(&[[s, 0.0, 0.0], [0.0, s, 0.0], [0.0, 0.0, 1.0]], &m_d);
    let r_n = tick.view.rotation.to_rotation();
    let r_d = display.to_rotation();
    let r_dt = [
        [r_d[0][0], r_d[1][0], r_d[2][0]],
        [r_d[0][1], r_d[1][1], r_d[2][1]],
        [r_d[0][2], r_d[1][2], r_d[2][2]],
    ];
    let h_ndc = mul3(&mul3(&mul3(&m_w, &r_n), &r_dt), &inv3(&m_d)?);
    // ndc -> uv: uv = (ndc * scale + offset) / size.
    let a = [
        [
            screen.scale[0] / screen.size[0],
            0.0,
            screen.offset[0] / screen.size[0],
        ],
        [
            0.0,
            screen.scale[1] / screen.size[1],
            screen.offset[1] / screen.size[1],
        ],
        [0.0, 0.0, 1.0],
    ];
    let h = mul3(&mul3(&a, &h_ndc), &inv3(&a)?);
    // Normalise so the bottom-right term is 1 (well conditioned in f32).
    let n = if h[2][2].abs() > 1e-12 { h[2][2] } else { 1.0 };
    let out = h.map(|r| r.map(|v| (v / n) as f32));
    out.iter().flatten().all(|v| v.is_finite()).then_some(out)
}

/// The display camera's rotation at `alpha` between frames N-1 (`prev`)
/// and N (`cur`): blended (one frame behind, like the replay) or, with
/// `ahead`, carried on past N along the same turn by `alpha` of a frame
/// (no added latency for turning, but it overshoots when the turn stops).
/// A cut gives N's rotation.
pub fn display_rotation(
    prev: &Persp,
    cur: &Persp,
    alpha: f32,
    ahead: bool,
    tuning: &Tuning,
) -> Quat {
    if prev.cut(cur, tuning.cut_distance, tuning.cut_angle, tuning.cut_zoom) {
        return cur.view.rotation;
    }
    let t = alpha.clamp(0.0, 1.0) as f64;
    let t = if ahead { 1.0 + t } else { t };
    prev.view.rotation.slerp(cur.view.rotation, t)
}

/// The constants the world draws (`..world_draws`) are drawn with into the
/// world image: every perspective block's clip `x` and `y` scaled by `s`.
pub fn overscan(
    model: &FrameModel,
    world_draws: usize,
    s: f64,
    constants: &dyn Fn(usize) -> Constants,
) -> Vec<Option<Constants>> {
    let mut out = vec![None; model.draws.len()];
    for (i, d) in model.draws.iter().enumerate().take(world_draws) {
        let blocks: Vec<_> = d
            .blocks
            .iter()
            .filter(|b| matches!(b.kind, Kind::Camera | Kind::World(_) | Kind::Local(_)))
            .collect();
        if blocks.is_empty() {
            continue;
        }
        let mut c = *constants(i);
        for b in blocks {
            let mut m = read4(&c, b.reg as usize, b.layout);
            for j in 0..4 {
                m[0][j] *= s;
                m[1][j] *= s;
            }
            write(&mut c, b.reg as usize, b.layout, 4, &m);
        }
        let mut block = crate::shader::spare_constants();
        *std::sync::Arc::get_mut(&mut block).expect("a spare block is unshared") = c;
        out[i] = Some(block);
    }
    out
}

/// The first draw that goes on top of the warped world: the first draw with
/// a projection of its own (the first-person weapon) after the world has
/// started, or the first draw after the last one that uses the camera (the
/// HUD), whichever comes first. Without a pass tag this is the rule; draws
/// after it replay unwarped.
pub fn overlay_start(model: &FrameModel) -> usize {
    let uses_camera = |i: usize| {
        model.draws[i]
            .blocks
            .iter()
            .any(|b| matches!(b.kind, Kind::Camera | Kind::World(_)))
    };
    let first_world = (0..model.draws.len()).find(|&i| uses_camera(i));
    let Some(first) = first_world else {
        return model.draws.len();
    };
    let last = (0..model.draws.len())
        .rev()
        .find(|&i| uses_camera(i))
        .unwrap_or(first);
    let own = (first..model.draws.len()).find(|&i| {
        !uses_camera(i)
            && model.draws[i]
                .blocks
                .iter()
                .any(|b| matches!(b.kind, Kind::Local(_)))
    });
    own.unwrap_or(last + 1).min(last + 1)
}

#[cfg(test)]
mod tests {
    use super::super::camera::build;
    use super::*;

    /// Where world point `x` lands in normalised texture coordinates for a
    /// camera matrix `m` (with the default screen mapping).
    fn uv(m: &Mat4, x: [f64; 3], screen: &Screen) -> [f64; 2] {
        let c: Vec<f64> = (0..4)
            .map(|i| m[i][0] * x[0] + m[i][1] * x[1] + m[i][2] * x[2] + m[i][3])
            .collect();
        let ndc = [c[0] / c[3], c[1] / c[3]];
        [
            (ndc[0] * screen.scale[0] + screen.offset[0]) / screen.size[0],
            (ndc[1] * screen.scale[1] + screen.offset[1]) / screen.size[1],
        ]
    }

    fn apply(h: &[[f32; 3]; 3], p: [f64; 2]) -> [f64; 2] {
        let q: Vec<f64> = (0..3)
            .map(|i| h[i][0] as f64 * p[0] + h[i][1] as f64 * p[1] + h[i][2] as f64)
            .collect();
        [q[0] / q[2], q[1] / q[2]]
    }

    fn screen() -> Screen {
        Screen {
            scale: [320.0, -240.0],
            offset: [320.0, 240.0],
            size: [640.0, 480.0],
        }
    }

    #[test]
    fn the_warp_samples_where_the_world_image_drew_the_point() {
        let eye = [3.0, -2.0, 1.5];
        let tick = Persp::split(&build::view_projection(1.2, eye, 0.3, 0.05)).unwrap();
        let display = Persp::split(&build::view_projection(1.2, eye, 0.42, -0.03)).unwrap();
        let s = overscan_scale();
        let h = homography(&tick, display.view.rotation, s, &screen()).unwrap();
        let mut world_image = tick.matrix();
        for j in 0..4 {
            world_image[0][j] *= s;
            world_image[1][j] *= s;
        }
        // Points in front of the display camera, near and far: a rotation
        // needs no depth.
        for x in [
            [10.0, 2.0, 1.0],
            [40.0, 20.0, 5.0],
            [6.0, -4.0, 0.0],
            [300.0, 100.0, 30.0],
        ] {
            let d = uv(&display.matrix(), x, &screen());
            let w = uv(&world_image, x, &screen());
            let got = apply(&h, d);
            assert!(
                (got[0] - w[0]).abs() < 1e-4 && (got[1] - w[1]).abs() < 1e-4,
                "{x:?}: {got:?} vs {w:?}"
            );
        }
    }

    #[test]
    fn no_turn_is_the_overscan_zoom() {
        let tick = Persp::split(&build::view_projection(1.2, [0.0; 3], 0.0, 0.0)).unwrap();
        let h = homography(&tick, tick.view.rotation, overscan_scale(), &screen()).unwrap();
        let c = apply(&h, [0.5, 0.5]);
        assert!((c[0] - 0.5).abs() < 1e-6 && (c[1] - 0.5).abs() < 1e-6);
        let e = apply(&h, [1.0, 1.0]);
        let s = overscan_scale();
        assert!((e[0] - (0.5 + 0.5 * s)).abs() < 1e-6 && (e[1] - (0.5 + 0.5 * s)).abs() < 1e-6);
    }

    #[test]
    fn display_rotations_blend_or_run_ahead_and_snap_on_cuts() {
        let t = Tuning::default();
        let a = Persp::split(&build::view_projection(1.2, [0.0; 3], 0.0, 0.0)).unwrap();
        let b = Persp::split(&build::view_projection(1.2, [0.0; 3], 0.2, 0.0)).unwrap();
        let ang = |q: Quat, p: &Persp| q.angle_to(p.view.rotation);
        assert!(ang(display_rotation(&a, &b, 0.0, false, &t), &a) < 1e-9);
        assert!(ang(display_rotation(&a, &b, 1.0, false, &t), &b) < 1e-9);
        let half = display_rotation(&a, &b, 0.5, false, &t);
        assert!((ang(half, &a) - 0.1).abs() < 1e-6);
        let ahead = display_rotation(&a, &b, 0.5, true, &t);
        assert!((ang(ahead, &a) - 0.3).abs() < 1e-6, "half a frame past N");
        let zoom = Persp::split(&build::view_projection(0.4, [0.0; 3], 0.2, 0.0)).unwrap();
        assert!(ang(display_rotation(&a, &zoom, 0.5, true, &t), &zoom) < 1e-9);
    }
}
