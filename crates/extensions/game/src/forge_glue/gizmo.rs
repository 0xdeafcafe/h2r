//! The Objects tool's three axes at the held or aimed object
//! (docs/game/forge-firefight-ui.md, "Markers in the world"): the ends of
//! the three segments the HUD kit's `forge.gizmo.x`, `.y` and `.z` world
//! marks draw, in world space, in the world's or the object's own basis as
//! the Axes setting says. The window's world-line pass projects them with
//! the scene's camera, as it does every world mark, so nothing here knows
//! the camera but its position.

/// The longest an arm is, in world units: near the camera the arms are
/// shorter, so they do not fill the view.
pub const ARM_MAX: f32 = 0.5;
/// The shortest, so an object at the camera's feet still has an arm.
pub const ARM_MIN: f32 = 0.05;
/// An arm's length as a share of the object's distance from the camera.
pub const ARM_SHARE: f32 = 0.14;

/// The gizmo: where the axes meet and the end of each arm, X, Y and Z.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Axes {
    pub origin: [f32; 3],
    pub ends: [[f32; 3]; 3],
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

impl Axes {
    /// The axes at `frame`, seen from `eye`: the object's own when `local`,
    /// else the world's. A frame or an eye that is not a number gives none.
    pub fn from_frame(frame: forge::Frame, local: bool, eye: [f32; 3]) -> Option<Axes> {
        let finite = frame
            .position
            .iter()
            .chain(&frame.rotation)
            .chain(&eye)
            .all(|v| v.is_finite());
        if !finite {
            return None;
        }
        let (x, z) = if local {
            live_telemetry::rotation_axes(frame.rotation.map(f64::from))
        } else {
            ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0])
        };
        let y = cross(z, x);
        let away = std::array::from_fn::<f32, 3, _>(|i| frame.position[i] - eye[i]);
        let distance = away.iter().map(|d| d * d).sum::<f32>().sqrt();
        let length = (distance * ARM_SHARE).clamp(ARM_MIN, ARM_MAX);
        let end =
            |axis: [f64; 3]| std::array::from_fn(|i| frame.position[i] + length * axis[i] as f32);
        Some(Axes {
            origin: frame.position,
            ends: [end(x), end(y), end(z)],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(position: [f32; 3], yaw: f32) -> forge::Frame {
        forge::Frame {
            position,
            rotation: [yaw, 0.0, 0.0],
        }
    }

    fn arm(a: &Axes, i: usize) -> [f32; 3] {
        std::array::from_fn(|k| a.ends[i][k] - a.origin[k])
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-4)
    }

    #[test]
    fn the_world_axes_ignore_the_turn_and_the_local_ones_follow_it() {
        let eye = [0.0; 3];
        let at = [2.0, 0.0, 0.0];
        let world = Axes::from_frame(frame(at, 0.0), false, eye).unwrap();
        let turned_world = Axes::from_frame(frame(at, 1.0), false, eye).unwrap();
        assert_eq!(world, turned_world, "World ignores the object's turn");
        let len = ARM_SHARE * 2.0;
        assert!(close(arm(&world, 0), [len, 0.0, 0.0]));
        assert!(close(arm(&world, 1), [0.0, len, 0.0]));
        assert!(close(arm(&world, 2), [0.0, 0.0, len]));
        let local = Axes::from_frame(frame(at, 0.0), true, eye).unwrap();
        assert!(close(arm(&local, 0), arm(&world, 0)), "no turn, same axes");
        // A quarter turn about the vertical: forward becomes lateral.
        let turned = Axes::from_frame(frame(at, std::f32::consts::FRAC_PI_2), true, eye).unwrap();
        assert!(close(arm(&turned, 0), arm(&local, 1)));
        assert!(close(arm(&turned, 2), arm(&local, 2)), "up stays up");
        // The three arms are at right angles and as long as each other.
        for a in [&world, &local, &turned] {
            let dot = |i: usize, j: usize| (0..3).map(|k| arm(a, i)[k] * arm(a, j)[k]).sum::<f32>();
            for (i, j) in [(0, 1), (0, 2), (1, 2)] {
                assert!(dot(i, j).abs() < 1e-5, "{a:?}");
            }
            assert!((dot(0, 0) - dot(1, 1)).abs() < 1e-5 && (dot(0, 0) - dot(2, 2)).abs() < 1e-5);
        }
    }

    #[test]
    fn the_arms_are_short_near_the_camera_and_capped_far_from_it() {
        let len = |distance: f32| {
            let a = Axes::from_frame(frame([distance, 0.0, 0.0], 0.0), false, [0.0; 3]).unwrap();
            arm(&a, 0)[0]
        };
        assert_eq!(len(0.0), ARM_MIN, "at the camera, the shortest");
        assert!((len(1.0) - ARM_SHARE).abs() < 1e-6, "in proportion");
        assert_eq!(len(10.0), ARM_MAX, "far, the longest");
    }

    #[test]
    fn a_frame_that_is_not_a_number_has_no_axes() {
        assert!(Axes::from_frame(frame([f32::NAN, 0.0, 0.0], 0.0), false, [0.0; 3]).is_none());
        assert!(Axes::from_frame(frame([1.0; 3], f32::INFINITY), true, [0.0; 3]).is_none());
        assert!(Axes::from_frame(frame([1.0; 3], 0.0), true, [f32::NAN; 3]).is_none());
    }
}
