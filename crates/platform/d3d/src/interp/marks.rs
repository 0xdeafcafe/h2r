//! The cameras a made frame drew its views with, for the window's world
//! marks (docs/game/high-framerate.md, "World marks on made frames").
//!
//! A mark is a world point the window projects over the finished frame
//! with the camera of the genuine image it was captured with. An
//! in-between frame draws the world with another camera: the blended one
//! ([`super::model::between`]) or, under E1, the tick camera turned to the
//! display rotation ([`super::reproject::display_camera`]). [`shown`] gives
//! that camera per view by the same rules the frame is made with, and
//! [`metadata`] turns it into a [`ViewShift`] per view (`C⁻¹ B`: the
//! source's camera `C`, the shown one `B`), so a mark lands where the world
//! under it is drawn.

use std::sync::Arc;

use super::backend::Mode;
use super::camera::Persp;
use super::math::{IDENTITY, inverse, max_diff, mul};
use super::model::{Between, FrameModel, Tuning, ViewKey, between};
use super::reproject;
use crate::present_camera::{Image, Present, ViewShift};

/// A view's camera on a made frame, and whether it is the newest frame's
/// own (nothing moved, a cut, or the blend's end).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shown {
    pub view: ViewKey,
    pub camera: Persp,
    pub own: bool,
}

/// The camera each view of `cur` is drawn with at `alpha` after `prev`,
/// made the `mode`'s way: the replay's blended camera per view (`cur`'s own
/// at `alpha >= 1`, on a cut, or when it did not move), or E1's turned tick
/// camera for its one view.
pub fn shown(
    mode: Mode,
    prev: &FrameModel,
    cur: &FrameModel,
    alpha: f32,
    tuning: &Tuning,
) -> Vec<Shown> {
    match mode {
        Mode::Replay => {
            let end = alpha >= 1.0 || alpha.is_nan();
            let t = alpha.max(0.0) as f64;
            cur.cameras
                .iter()
                .map(|c| {
                    let blended = match prev.camera(c.view) {
                        Some(p) if !end => match between(p, c, t, tuning) {
                            Between::Blend(b) => Some(b),
                            Between::Still | Between::Cut => None,
                        },
                        _ => None,
                    };
                    Shown {
                        view: c.view,
                        camera: blended.unwrap_or(c.camera),
                        own: blended.is_none(),
                    }
                })
                .collect()
        }
        Mode::Reproject { ahead } => {
            let (Some(p), Some(c)) = (prev.cameras.first(), cur.cameras.first()) else {
                return Vec::new();
            };
            let rotation = reproject::display_rotation(&p.camera, &c.camera, alpha, ahead, tuning);
            vec![Shown {
                view: c.view,
                camera: reproject::display_camera(&c.camera, rotation),
                own: rotation == c.camera.view.rotation,
            }]
        }
    }
}

/// A shift this close to the identity (rounding in `C⁻¹ C`) is the
/// identity.
const IDENTITY_TOLERANCE: f64 = 1e-9;

/// How `view` sits on its target: top, left, bottom, right as fractions.
fn viewport(view: &ViewKey) -> [f32; 4] {
    let (w, h) = (view.target.0.max(1) as f32, view.target.1.max(1) as f32);
    [
        view.y as f32 / h,
        view.x as f32 / w,
        (view.y + view.height) as f32 / h,
        (view.x + view.width) as f32 / w,
    ]
}

/// The shift that moves `tick`'s camera for `shown`'s view onto the one
/// it was drawn with, or `None` when `tick` has no camera for that view
/// or its matrix does not invert.
fn shift(tick: &FrameModel, shown: &Shown) -> Option<ViewShift> {
    let c = tick.camera(shown.view)?;
    let world = mul(&inverse(&c.camera.matrix())?, &shown.camera.matrix());
    Some(ViewShift {
        viewport: viewport(&shown.view),
        world: if max_diff(&world, &IDENTITY) <= IDENTITY_TOLERANCE {
            IDENTITY
        } else {
            world
        },
    })
}

/// The present metadata of a frame made at `alpha` from `prev` and `cur`
/// (their models and the genuine images their marks were captured with).
///
/// The marks come from the newest image, except while a blended replay is
/// nearer the frame before (`alpha < 0.5` with a view that blends): then
/// from that one, so what a mark shows lags the world by at most half a
/// tick. Either way each view's shift moves the source's camera onto the
/// one the frame was drawn with.
pub fn metadata(
    mode: Mode,
    prev: (&FrameModel, Option<&Arc<Image>>),
    cur: (&FrameModel, Option<&Arc<Image>>),
    alpha: f32,
    tuning: &Tuning,
) -> Present {
    let shown = shown(mode, prev.0, cur.0, alpha, tuning);
    let blends = shown.iter().any(|s| !s.own);
    let (model, source) = match mode {
        Mode::Replay if blends && alpha < 0.5 && prev.1.is_some() => prev,
        _ => cur,
    };
    Present::Synthetic {
        source: source.cloned(),
        shifts: shown.iter().filter_map(|s| shift(model, s)).collect(),
        fraction: alpha,
        reference: false,
    }
}

#[cfg(test)]
mod tests {
    use super::super::camera::build;
    use super::super::math::Mat4;
    use super::super::model::ViewCamera;
    use super::*;

    fn view() -> ViewKey {
        ViewKey {
            x: 0,
            y: 0,
            width: 640,
            height: 480,
            target: (640, 480),
        }
    }

    fn frame(eye: [f64; 3], yaw: f64, fov: f64) -> FrameModel {
        let m = build::view_projection(fov, eye, yaw, 0.05);
        let raw = std::array::from_fn(|i| (m[i / 4][i % 4] as f32).to_bits());
        FrameModel {
            cameras: vec![ViewCamera {
                view: view(),
                camera: Persp::split(&m).unwrap(),
                raw,
            }],
            draws: Vec::new(),
        }
    }

    fn image(n: u32) -> Arc<Image> {
        Arc::new(Image {
            views: [None; 4],
            payload: Arc::new(n),
        })
    }

    /// Where world point `x` lands in normalised device coordinates under
    /// the row-major matrix `m`.
    fn ndc(m: &Mat4, x: [f64; 3]) -> [f64; 2] {
        let c: Vec<f64> = (0..4)
            .map(|i| m[i][0] * x[0] + m[i][1] * x[1] + m[i][2] * x[2] + m[i][3])
            .collect();
        [c[0] / c[3], c[1] / c[3]]
    }

    /// A window's mark camera for the tick camera `c`: column-major, in a
    /// clip space of its own (`F * C`: x halved for the aspect, y flipped,
    /// depth remapped), so the test does not lean on the two conventions
    /// being the same.
    fn mark_camera(c: &Persp) -> [[f32; 4]; 4] {
        let f: Mat4 = [
            [0.5, 0.0, 0.0, 0.0],
            [0.0, -1.0, 0.0, 0.0],
            [0.0, 0.0, 0.5, 0.25],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let m = mul(&f, &c.matrix());
        std::array::from_fn(|column| std::array::from_fn(|row| m[row][column] as f32))
    }

    /// The point through a column-major mark camera, back in the test's
    /// device coordinates (`F⁻¹` on x and y).
    fn mark_ndc(vp: &[[f32; 4]; 4], x: [f64; 3]) -> [f64; 2] {
        let c: Vec<f64> = (0..4)
            .map(|i| {
                f64::from(vp[0][i]) * x[0]
                    + f64::from(vp[1][i]) * x[1]
                    + f64::from(vp[2][i]) * x[2]
                    + f64::from(vp[3][i])
            })
            .collect();
        [c[0] / c[3] / 0.5, c[1] / c[3] / -1.0]
    }

    /// In front of every test camera (they look down world -z, `build`).
    const POINTS: [[f64; 3]; 3] = [[1.0, 0.5, -5.0], [3.0, -2.0, -20.0], [-2.0, 1.0, -8.0]];

    fn assert_lands(shifted: &[[f32; 4]; 4], drawn: &Mat4) {
        for x in POINTS {
            let (got, want) = (mark_ndc(shifted, x), ndc(drawn, x));
            assert!(
                (got[0] - want[0]).abs() < 1e-4 && (got[1] - want[1]).abs() < 1e-4,
                "{x:?}: the mark at {got:?}, the world at {want:?}"
            );
        }
    }

    fn synthetic(p: Present) -> (Option<u32>, Vec<ViewShift>, f32) {
        let Present::Synthetic {
            source,
            shifts,
            fraction,
            ..
        } = p
        else {
            panic!("a made frame is synthetic");
        };
        let n = source.map(|i| *i.payload.downcast_ref::<u32>().unwrap());
        (n, shifts, fraction)
    }

    #[test]
    fn a_mark_halfway_lands_where_the_replay_draws_the_world() {
        let t = Tuning::default();
        let (prev, cur) = (
            frame([0.0, 0.0, 1.0], 0.3, 1.2),
            frame([0.4, 0.1, 1.0], 0.5, 1.2),
        );
        let (pi, ci) = (image(1), image(2));
        for alpha in [0.25, 0.5, 0.75] {
            let p = metadata(
                Mode::Replay,
                (&prev, Some(&pi)),
                (&cur, Some(&ci)),
                alpha,
                &t,
            );
            let (source, shifts, fraction) = synthetic(p);
            assert_eq!(fraction, alpha);
            // The replay's camera: the same rule substitute uses.
            let Between::Blend(drawn) =
                between(&prev.cameras[0], &cur.cameras[0], alpha as f64, &t)
            else {
                panic!("a small turn blends");
            };
            let (tick, n) = if alpha < 0.5 { (&prev, 1) } else { (&cur, 2) };
            assert_eq!(source, Some(n), "the nearer tick's marks at {alpha}");
            let s = ViewShift::find(&shifts, [0.0, 0.0, 1.0, 1.0], 1).unwrap();
            assert_lands(
                &s.apply(mark_camera(&tick.cameras[0].camera)),
                &drawn.matrix(),
            );
        }
        // Without the shift the mark would sit on the tick camera's
        // picture, off the world under it.
        let (_, shifts, _) = synthetic(metadata(
            Mode::Replay,
            (&prev, Some(&pi)),
            (&cur, Some(&ci)),
            0.5,
            &t,
        ));
        assert_ne!(shifts[0].world, IDENTITY);
        let x = POINTS[0];
        let frozen = mark_ndc(&mark_camera(&cur.cameras[0].camera), x);
        let blended = prev.cameras[0]
            .camera
            .lerp_camera(&cur.cameras[0].camera, 0.5);
        assert!((frozen[0] - ndc(&blended.matrix(), x)[0]).abs() > 1e-3);
    }

    #[test]
    fn a_zoom_blend_moves_the_marks_with_the_projection_too() {
        let t = Tuning::default();
        // 2% narrower: under the cut's 5%, so the projection blends.
        let (prev, cur) = (frame([0.0; 3], 0.3, 1.2), frame([0.0; 3], 0.32, 1.176));
        let ci = image(2);
        let (_, shifts, _) = synthetic(metadata(
            Mode::Replay,
            (&prev, None),
            (&cur, Some(&ci)),
            0.4,
            &t,
        ));
        let drawn = prev.cameras[0]
            .camera
            .lerp_camera(&cur.cameras[0].camera, 0.4);
        // The frame before has no image: the newest's marks, shifted back.
        assert_lands(
            &shifts[0].apply(mark_camera(&cur.cameras[0].camera)),
            &drawn.matrix(),
        );
    }

    #[test]
    fn a_cut_or_the_end_of_the_blend_draws_the_newest_camera() {
        let t = Tuning::default();
        let prev = frame([0.0; 3], 0.3, 1.2);
        let (pi, ci) = (image(1), image(2));
        // A jump of 30 units is a cut: the replay snaps to the newest camera.
        let cut = frame([30.0, 0.0, 0.0], 0.3, 1.2);
        for (cur, alpha) in [(&cut, 0.25), (&frame([0.4, 0.0, 0.0], 0.5, 1.2), 1.0)] {
            let (source, shifts, _) = synthetic(metadata(
                Mode::Replay,
                (&prev, Some(&pi)),
                (cur, Some(&ci)),
                alpha,
                &t,
            ));
            assert_eq!(source, Some(2));
            assert_eq!(shifts.len(), 1);
            assert_eq!(shifts[0].world, IDENTITY);
        }
        // A camera that did not move between the ticks stands as well.
        let (source, shifts, _) = synthetic(metadata(
            Mode::Replay,
            (&prev, Some(&pi)),
            (&prev, Some(&ci)),
            0.25,
            &t,
        ));
        assert_eq!((source, shifts[0].world), (Some(2), IDENTITY));
    }

    #[test]
    fn under_reprojection_marks_follow_the_turned_camera() {
        let t = Tuning::default();
        let (prev, cur) = (
            frame([0.0, 0.0, 1.0], 0.3, 1.2),
            frame([0.5, 0.0, 1.0], 0.4, 1.2),
        );
        let ci = image(2);
        for ahead in [false, true] {
            let mode = Mode::Reproject { ahead };
            let (source, shifts, _) = synthetic(metadata(
                mode,
                (&prev, Some(&image(1))),
                (&cur, Some(&ci)),
                0.5,
                &t,
            ));
            // E1 shows the newest world turned: always its marks.
            assert_eq!(source, Some(2));
            let rotation = reproject::display_rotation(
                &prev.cameras[0].camera,
                &cur.cameras[0].camera,
                0.5,
                ahead,
                &t,
            );
            let drawn = reproject::display_camera(&cur.cameras[0].camera, rotation);
            // The eye stays the tick's: only the turn moves the marks.
            let e = drawn.eye();
            assert!((e[0] - 0.5).abs() < 1e-9 && e[1].abs() < 1e-9);
            assert_lands(
                &shifts[0].apply(mark_camera(&cur.cameras[0].camera)),
                &drawn.matrix(),
            );
        }
        // A cut under E1 holds the newest rotation: no shift.
        let cut = frame([0.5, 0.0, 1.0], 2.0, 1.2);
        let (_, shifts, _) = synthetic(metadata(
            Mode::Reproject { ahead: false },
            (&prev, None),
            (&cut, Some(&ci)),
            0.5,
            &t,
        ));
        assert!(max_diff(&shifts[0].world, &IDENTITY) <= IDENTITY_TOLERANCE);
    }

    #[test]
    fn split_views_shift_each_on_its_own_rectangle() {
        let view = ViewKey {
            x: 0,
            y: 240,
            width: 640,
            height: 240,
            target: (640, 480),
        };
        assert_eq!(super::viewport(&view), [0.5, 0.0, 1.0, 1.0]);
    }
}
