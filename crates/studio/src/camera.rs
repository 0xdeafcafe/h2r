//! The viewport's camera controls, game-editor style, as plain maths over
//! [`scenario_edit::geometry::Camera`] (no UI toolkit, unit-tested here):
//!
//! - **Fly** while the right button is held: the mouse looks around from
//!   where the eye is, W/A/S/D move, Q/E go down and up, Shift is
//!   [`FAST`] times faster and the wheel changes the speed. Moving eases
//!   in and out over [`EASE`] seconds, the same at any frame rate.
//! - **Orbit** on Alt + left drag, about a pivot: the selection, or the
//!   surface under the cursor when nothing is selected.
//! - **Pan** on middle drag, **dolly** on the wheel (through the focus
//!   point when it gets close, rather than stopping at it).
//! - **Frame** ([`frame_sphere`]) and **focus** ([`focus_point`]) give a
//!   camera to go to; the controller glides there over [`GLIDE`] seconds.
use scenario_edit::geometry::{Camera, Vec3, add, cross, dot, normalize, scale, sub};

/// Fly speed when the editor starts, in world units a second.
pub const DEFAULT_SPEED: f32 = 12.0;
/// The slowest and fastest fly speeds the wheel reaches.
pub const MIN_SPEED: f32 = 0.5;
pub const MAX_SPEED: f32 = 1000.0;
/// One wheel notch multiplies the fly speed by this.
pub const SPEED_STEP: f32 = 1.25;
/// Shift multiplies the fly speed by this.
pub const FAST: f32 = 4.0;
/// How long flying takes to reach (or lose) most of its speed: the time
/// constant of the easing, in seconds.
pub const EASE: f32 = 0.08;
/// How long a glide to a framed or focused camera takes, in seconds.
pub const GLIDE: f32 = 0.25;
/// Radians per point of mouse movement, looking and orbiting.
pub const LOOK: f32 = 0.006;
/// The share of the distance to the focus one wheel notch dollies.
pub const DOLLY_STEP: f32 = 0.15;
/// Nearer than this, dollying pushes the focus point ahead instead.
pub const MIN_DISTANCE: f32 = 1.0;

/// One frame of input for the camera.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Input {
    /// Seconds since the last frame.
    pub dt: f32,
    /// Mouse movement in points.
    pub mouse: [f32; 2],
    /// Wheel notches (positive away from the user).
    pub wheel: f32,
    /// The right button is held (it went down over the viewport).
    pub fly: bool,
    /// Alt + left drag is orbiting.
    pub orbit: bool,
    /// The middle button is held (it went down over the viewport).
    pub pan: bool,
    /// Held movement keys: right (D - A), up (E - Q), forward (W - S).
    pub keys: [f32; 3],
    /// Shift is held.
    pub fast: bool,
    /// The viewport's height in points (for panning).
    pub height: f32,
}

/// The camera's state between frames.
#[derive(Clone, Debug, PartialEq)]
pub struct Controller {
    /// Fly speed, world units a second (before Shift).
    pub speed: f32,
    /// Glide to cameras changed from outside ([`Self::update`]); off for
    /// headless screenshots, which want the camera where it was put.
    pub smooth: bool,
    /// Current fly velocity in camera space (right, up, forward).
    velocity: Vec3,
    /// The pivot of the orbit in progress.
    pivot: Option<Vec3>,
    /// The glide in progress.
    glide: Option<Glide>,
    /// The camera as this controller last left it.
    last: Option<Camera>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Glide {
    from: Camera,
    to: Camera,
    t: f32,
}

impl Default for Controller {
    fn default() -> Self {
        Self::new()
    }
}

impl Controller {
    pub fn new() -> Self {
        Self {
            speed: DEFAULT_SPEED,
            smooth: true,
            velocity: [0.0; 3],
            pivot: None,
            glide: None,
            last: None,
        }
    }

    /// The speed flying moves at now, with Shift.
    pub fn effective_speed(&self, fast: bool) -> f32 {
        if fast { self.speed * FAST } else { self.speed }
    }

    /// Whether the camera is still moving on its own (a glide, or flying
    /// easing out): the UI keeps drawing frames until it stops.
    pub fn moving(&self) -> bool {
        self.glide.is_some() || self.velocity.iter().any(|v| v.abs() > 1e-3)
    }

    /// Forget the camera's history (another map opened): the next change
    /// from outside is taken as it is, not glided to.
    pub fn reset(&mut self) {
        self.velocity = [0.0; 3];
        self.pivot = None;
        self.glide = None;
        self.last = None;
    }

    /// Glide to `to` (when smoothing; else jump there).
    pub fn go_to(&mut self, cam: &mut Camera, to: Camera) {
        if self.smooth {
            self.glide = Some(Glide {
                from: *cam,
                to,
                t: 0.0,
            });
        } else {
            *cam = to;
        }
        self.last = Some(*cam);
    }

    /// Start orbiting about `pivot` (Alt + left button went down).
    pub fn begin_orbit(&mut self, pivot: Vec3) {
        self.pivot = Some(pivot);
    }

    /// Apply one frame of input to `cam`. A camera changed since the last
    /// call by someone else (Focus, Frame, a script) is glided to.
    pub fn update(&mut self, cam: &mut Camera, input: &Input) {
        let dt = input.dt.clamp(0.0, 0.25);
        if let Some(last) = self.last
            && last != *cam
            && self.glide.is_none_or(|g| g.to != *cam)
        {
            let to = *cam;
            *cam = last;
            self.go_to(cam, to);
        }
        // Any input of the user's own ends a glide where it is.
        let user = input.fly
            || input.orbit
            || input.pan
            || input.wheel != 0.0
            || input.keys.iter().any(|&k| k != 0.0);
        if user {
            self.glide = None;
        }
        if let Some(g) = &mut self.glide {
            g.t = (g.t + dt / GLIDE).min(1.0);
            *cam = blend(&g.from, &g.to, ease(g.t));
            if g.t >= 1.0 {
                self.glide = None;
            }
        }

        if input.fly {
            look(cam, -input.mouse[0] * LOOK, input.mouse[1] * LOOK);
            if input.wheel != 0.0 {
                self.speed =
                    (self.speed * SPEED_STEP.powf(input.wheel)).clamp(MIN_SPEED, MAX_SPEED);
            }
        } else if input.wheel != 0.0 {
            dolly(cam, input.wheel);
        }
        if input.orbit {
            if let Some(p) = self.pivot {
                orbit_about(cam, p, -input.mouse[0] * LOOK, input.mouse[1] * LOOK);
            }
        } else {
            self.pivot = None;
        }
        if input.pan {
            cam.pan(input.mouse[0], input.mouse[1], input.height);
        }

        // Fly velocity eases toward the keys' target, frame-rate free.
        let goal = if input.fly {
            scale(input.keys, self.effective_speed(input.fast))
        } else {
            [0.0; 3]
        };
        let k = 1.0 - (-dt / EASE).exp();
        self.velocity = add(self.velocity, scale(sub(goal, self.velocity), k));
        if self.velocity.iter().all(|v| v.abs() < 1e-3) && goal == [0.0; 3] {
            self.velocity = [0.0; 3];
        }
        let step = scale(self.velocity, dt);
        if step != [0.0; 3] {
            fly(cam, step);
        }
        self.last = Some(*cam);
    }
}

/// Smoothstep.
pub fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The angle `b` taken the short way round from `a`.
fn near_angle(a: f32, b: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    a + (b - a + std::f32::consts::PI).rem_euclid(tau) - std::f32::consts::PI
}

/// A camera part way (`t`, 0..1) from `a` to `b`: the focus point and
/// distance straight, yaw the short way round.
pub fn blend(a: &Camera, b: &Camera, t: f32) -> Camera {
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    let yaw_b = near_angle(a.yaw, b.yaw);
    let yaw = if t >= 1.0 { b.yaw } else { lerp(a.yaw, yaw_b) };
    Camera {
        target: add(a.target, scale(sub(b.target, a.target), t)),
        yaw,
        pitch: lerp(a.pitch, b.pitch),
        // Distance in log space, so a long zoom feels even.
        distance: if t >= 1.0 {
            b.distance
        } else {
            (a.distance.max(1e-3).ln() * (1.0 - t) + b.distance.max(1e-3).ln() * t).exp()
        },
        fov_y: lerp(a.fov_y, b.fov_y),
    }
}

/// Turn the view from where the eye is (yaw `dx`, pitch `dy` radians):
/// the eye stays, the focus point swings round it.
pub fn look(cam: &mut Camera, dx: f32, dy: f32) {
    let eye = cam.eye();
    cam.orbit(dx, dy);
    cam.target = add(eye, scale(cam.forward(), cam.distance));
}

/// Aim the camera at `p` without moving the eye.
pub fn retarget(cam: &mut Camera, p: Vec3) {
    let eye = cam.eye();
    let d = sub(p, eye);
    let len = dot(d, d).sqrt();
    if len < 1e-4 {
        return;
    }
    let f = scale(d, 1.0 / len);
    // forward = [-cos yaw cos pitch, -sin yaw cos pitch, -sin pitch]
    cam.pitch = (-f[2]).clamp(-1.0, 1.0).asin().clamp(-1.5, 1.5);
    cam.yaw = (-f[1]).atan2(-f[0]);
    cam.distance = len;
    cam.target = p;
}

/// Orbit the eye about `pivot` by yaw `dx` and pitch `dy` radians.
pub fn orbit_about(cam: &mut Camera, pivot: Vec3, dx: f32, dy: f32) {
    if dot(sub(cam.target, pivot), sub(cam.target, pivot)) > 1e-8 {
        retarget(cam, pivot);
    }
    cam.orbit(dx, dy);
}

/// Dolly by wheel notches: toward the focus point by a share of the
/// distance; once close, the focus point is pushed ahead so the wheel
/// keeps moving the eye.
pub fn dolly(cam: &mut Camera, notches: f32) {
    let step = cam.distance.max(MIN_DISTANCE) * DOLLY_STEP * notches;
    let want = cam.distance - step;
    if want >= MIN_DISTANCE {
        cam.distance = want.min(5000.0);
    } else {
        let over = MIN_DISTANCE - want;
        cam.distance = MIN_DISTANCE;
        cam.target = add(cam.target, scale(cam.forward(), over));
    }
}

/// Move the eye and focus point together in camera space: right, up
/// (world z, so flying level stays level), forward (where the camera
/// looks).
pub fn fly(cam: &mut Camera, step: Vec3) {
    let f = cam.forward();
    let r = normalize(cross(f, [0.0, 0.0, 1.0]));
    cam.target = add(
        cam.target,
        add(
            scale(r, step[0]),
            add(scale(f, step[2]), [0.0, 0.0, step[1]]),
        ),
    );
}

/// A camera looking at a sphere (`center`, `radius`) the way `cam` looks,
/// near enough that it fills most of the view.
pub fn frame_sphere(cam: &Camera, center: Vec3, radius: f32) -> Camera {
    let half = (cam.fov_y * 0.5).max(0.1);
    let distance = (radius.max(0.25) / half.sin() * 1.15).max(2.0);
    Camera {
        target: center,
        distance,
        ..*cam
    }
}

/// A camera looking at `p` from where `cam` is, brought in to no further
/// than `max_distance` (double-clicking a surface).
pub fn focus_point(cam: &Camera, p: Vec3, max_distance: f32) -> Camera {
    let mut c = *cam;
    retarget(&mut c, p);
    c.distance = c.distance.min(max_distance).max(MIN_DISTANCE);
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3, eps: f32) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() <= eps)
    }

    fn cam() -> Camera {
        Camera {
            target: [10.0, 5.0, 2.0],
            yaw: 0.3,
            pitch: 0.4,
            distance: 20.0,
            ..Camera::default()
        }
    }

    #[test]
    fn looking_turns_the_view_about_the_eye() {
        let mut c = cam();
        let eye = c.eye();
        look(&mut c, 0.5, -0.2);
        assert!(close(c.eye(), eye, 1e-4));
        assert!((c.yaw - 0.8).abs() < 1e-6);
        assert!((c.pitch - 0.2).abs() < 1e-6);
        // Pitch stops short of straight up and down.
        look(&mut c, 0.0, 10.0);
        assert!(c.pitch <= 1.5);
        assert!(close(c.eye(), eye, 1e-3));
    }

    #[test]
    fn retargeting_keeps_the_eye_and_looks_at_the_point() {
        let mut c = cam();
        let eye = c.eye();
        let p = [-3.0, 8.0, -1.0];
        retarget(&mut c, p);
        assert!(close(c.eye(), eye, 1e-3));
        assert!(close(c.target, p, 1e-6));
        let d = sub(p, eye);
        assert!((c.distance - dot(d, d).sqrt()).abs() < 1e-4);
    }

    #[test]
    fn orbiting_keeps_the_pivot_and_its_distance() {
        let mut c = cam();
        let pivot = [0.0, 0.0, 0.0];
        let eye = c.eye();
        let r = dot(sub(eye, pivot), sub(eye, pivot)).sqrt();
        // The first step re-aims at the pivot without jumping the eye.
        orbit_about(&mut c, pivot, 0.0, 0.0);
        assert!(close(c.eye(), eye, 1e-3));
        orbit_about(&mut c, pivot, 1.0, 0.3);
        let after = c.eye();
        assert!((dot(sub(after, pivot), sub(after, pivot)).sqrt() - r).abs() < 1e-3);
        assert!(close(c.target, pivot, 1e-6));
        assert!(!close(after, eye, 0.1));
    }

    #[test]
    fn dollying_moves_in_and_through_the_focus_point() {
        let mut c = cam();
        dolly(&mut c, 1.0);
        assert!((c.distance - 17.0).abs() < 1e-4);
        dolly(&mut c, -1.0);
        assert!(c.distance > 17.0);
        // Close in, the eye keeps going forward and the focus point moves.
        let mut c = Camera {
            distance: 1.2,
            ..cam()
        };
        let eye = c.eye();
        let f = c.forward();
        dolly(&mut c, 3.0);
        assert_eq!(c.distance, MIN_DISTANCE);
        assert!(dot(sub(c.eye(), eye), f) > 0.0);
        assert!(dot(sub(c.target, cam().target), f) > 0.0);
    }

    #[test]
    fn flying_moves_the_eye_level_and_forward() {
        let mut c = cam();
        let eye = c.eye();
        fly(&mut c, [0.0, 0.0, 2.0]);
        let moved = sub(c.eye(), eye);
        assert!((dot(moved, c.forward()) - 2.0).abs() < 1e-4);
        let mut c = cam();
        fly(&mut c, [3.0, 1.0, 0.0]);
        let moved = sub(c.eye(), eye);
        assert!((moved[2] - 1.0).abs() < 1e-4, "up is world z");
        assert!(((moved[0] * moved[0] + moved[1] * moved[1]).sqrt() - 3.0).abs() < 1e-4);
    }

    /// Flying with W held for a second covers the same ground at 30 and
    /// 240 frames a second, and eases in rather than jumping.
    #[test]
    fn flying_is_the_same_at_any_frame_rate() {
        let run = |fps: u32| {
            let mut c = cam();
            let mut ctl = Controller::new();
            let eye = c.eye();
            let input = Input {
                dt: 1.0 / fps as f32,
                fly: true,
                keys: [0.0, 0.0, 1.0],
                ..Input::default()
            };
            let mut first = None;
            for i in 0..fps {
                ctl.update(&mut c, &input);
                if i == 0 {
                    first = Some(dot(sub(c.eye(), eye), c.forward()));
                }
            }
            (dot(sub(c.eye(), eye), c.forward()), first.unwrap())
        };
        let (slow, slow_first) = run(30);
        let (fast, _) = run(240);
        assert!((slow - fast).abs() / fast < 0.03, "{slow} vs {fast}");
        // Nearly the full speed for the second, less the easing in.
        assert!(
            fast > DEFAULT_SPEED * 0.85 && fast < DEFAULT_SPEED,
            "{fast}"
        );
        // The first frame does not jump to full speed.
        assert!(slow_first < DEFAULT_SPEED / 30.0 * 0.9, "{slow_first}");
    }

    #[test]
    fn letting_go_eases_out_and_stops() {
        let mut c = cam();
        let mut ctl = Controller::new();
        let held = Input {
            dt: 1.0 / 60.0,
            fly: true,
            keys: [1.0, 0.0, 0.0],
            ..Input::default()
        };
        for _ in 0..60 {
            ctl.update(&mut c, &held);
        }
        let released = Input {
            dt: 1.0 / 60.0,
            ..Input::default()
        };
        let before = c.eye();
        ctl.update(&mut c, &released);
        assert!(ctl.moving());
        assert!(!close(c.eye(), before, 1e-4), "it coasts a little");
        for _ in 0..120 {
            ctl.update(&mut c, &released);
        }
        assert!(!ctl.moving());
    }

    #[test]
    fn shift_flies_faster_and_the_wheel_sets_the_speed_while_flying() {
        let mut ctl = Controller::new();
        assert_eq!(ctl.effective_speed(true), DEFAULT_SPEED * FAST);
        let mut c = cam();
        let d = c.distance;
        ctl.update(
            &mut c,
            &Input {
                dt: 0.016,
                fly: true,
                wheel: 2.0,
                ..Input::default()
            },
        );
        assert!((ctl.speed - DEFAULT_SPEED * SPEED_STEP * SPEED_STEP).abs() < 1e-4);
        assert_eq!(c.distance, d, "the wheel does not dolly while flying");
        for _ in 0..100 {
            ctl.update(
                &mut c,
                &Input {
                    dt: 0.016,
                    fly: true,
                    wheel: 5.0,
                    ..Input::default()
                },
            );
        }
        assert_eq!(ctl.speed, MAX_SPEED);
        // Without the right button the wheel dollies.
        ctl.update(
            &mut c,
            &Input {
                dt: 0.016,
                wheel: 1.0,
                ..Input::default()
            },
        );
        assert!(c.distance < d);
    }

    #[test]
    fn an_orbit_holds_its_pivot_until_released() {
        let mut c = cam();
        let mut ctl = Controller::new();
        ctl.begin_orbit([0.0; 3]);
        let drag = Input {
            dt: 0.016,
            orbit: true,
            mouse: [40.0, 10.0],
            ..Input::default()
        };
        ctl.update(&mut c, &drag);
        assert!(close(c.target, [0.0; 3], 1e-6));
        let eye = c.eye();
        ctl.update(&mut c, &drag);
        assert!(!close(c.eye(), eye, 1e-3), "still orbiting");
        // Released: a new drag has no pivot until one is given.
        ctl.update(&mut c, &Input::default());
        let before = c;
        ctl.update(&mut c, &drag);
        assert_eq!(c, before);
    }

    #[test]
    fn a_camera_set_from_outside_is_glided_to_in_a_quarter_second() {
        let mut c = cam();
        let mut ctl = Controller::new();
        let idle = Input {
            dt: 1.0 / 60.0,
            ..Input::default()
        };
        ctl.update(&mut c, &idle);
        // Focus: someone else sets the camera.
        let to = Camera {
            target: [100.0, 0.0, 0.0],
            distance: 5.0,
            ..c
        };
        c = to;
        ctl.update(&mut c, &idle);
        assert!(c.target[0] > 10.0 && c.target[0] < 100.0, "{:?}", c.target);
        assert!(ctl.moving());
        for _ in 0..20 {
            ctl.update(&mut c, &idle);
        }
        assert_eq!(c, to);
        assert!(!ctl.moving());
        // Not smoothing (headless): it jumps.
        let mut ctl = Controller {
            smooth: false,
            ..Controller::new()
        };
        let mut c = cam();
        ctl.update(&mut c, &idle);
        c = to;
        ctl.update(&mut c, &idle);
        assert_eq!(c, to);
    }

    #[test]
    fn glides_take_the_short_way_round() {
        let a = Camera { yaw: 3.0, ..cam() };
        let b = Camera { yaw: -3.0, ..cam() };
        let mid = blend(&a, &b, 0.5);
        // 3 to -3 the short way passes through pi, not 0.
        assert!(mid.yaw.cos() < -0.9, "{}", mid.yaw);
        assert_eq!(blend(&a, &b, 1.0), b);
        assert_eq!(blend(&a, &b, 0.0).target, a.target);
    }

    #[test]
    fn framing_fits_the_sphere_and_focus_brings_the_eye_in() {
        let c = cam();
        let f = frame_sphere(&c, [1.0, 2.0, 3.0], 4.0);
        assert_eq!(f.target, [1.0, 2.0, 3.0]);
        assert_eq!((f.yaw, f.pitch), (c.yaw, c.pitch));
        // The sphere's silhouette fits in the vertical field of view.
        assert!((4.0 / f.distance).asin() < c.fov_y * 0.5);
        assert!(f.distance < 4.0 / (c.fov_y * 0.5).sin() * 1.3);
        // A tiny glyph still gets some room.
        assert!(frame_sphere(&c, [0.0; 3], 0.0).distance >= 2.0);
        let p = [50.0, 50.0, 0.0];
        let g = focus_point(&c, p, 10.0);
        assert_eq!(g.target, p);
        assert_eq!(g.distance, 10.0);
    }
}
