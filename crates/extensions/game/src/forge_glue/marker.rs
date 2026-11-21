//! The monitor marker: a ring where a monitor is, and the place its
//! player's name goes (docs/game/forge-monitor-appearance.md, "Which
//! model"). It is projected through the sealed world camera by the same
//! function as the gizmo's axes ([`super::gizmo::pixel_of`]), so a marker
//! and a gizmo at one frame are at one pixel. Only the compositor sees
//! these pixels.
//!
//! A monitor never draws its own marker in its own view: the camera is
//! inside it. The marker is for another seat's monitor, drawn from that
//! seat's pose (slice ML7); an offline session has no other seat.
#[cfg(any(feature = "desktop-runtime", test))]
use frame_scene::RenderCamera;

#[cfg(any(feature = "desktop-runtime", test))]
use super::gizmo::pixel_of;

/// The ring's radius in world units.
#[cfg(any(feature = "desktop-runtime", test))]
const RING: f32 = 0.4;
/// Gap between the ring and the name, in canvas pixels.
#[cfg(any(feature = "desktop-runtime", test))]
const GAP: i32 = 4;

/// A projected marker in the HUD's 640 by 480 canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Marker {
    /// Where the monitor is.
    pub centre: [i32; 2],
    /// The ring's radius in pixels, at least 2.
    pub radius: i32,
    /// Where the middle of the player's name goes: above the ring.
    pub label: [i32; 2],
    /// The ring's colour: the look's primary, read from the loaded map.
    pub colour: [u8; 3],
}

/// The marker of a monitor at `frame`, through the last frame's world
/// view. `None` where there is no view or the monitor is out of it.
pub fn project(frame: forge::Frame, colour: [u8; 3]) -> Option<Marker> {
    #[cfg(feature = "desktop-runtime")]
    {
        let scene = render_bridge::scene_step::last()?;
        let view = scene
            .views
            .iter()
            .find(|v| v.key.kind == frame_scene::ViewKind::Game && v.key.view == 0)?;
        Marker::from_frame(frame, &view.camera, colour)
    }
    #[cfg(not(feature = "desktop-runtime"))]
    {
        let _ = (frame, colour);
        None
    }
}

impl Marker {
    #[cfg(any(feature = "desktop-runtime", test))]
    pub(super) fn from_frame(
        frame: forge::Frame,
        camera: &RenderCamera,
        colour: [u8; 3],
    ) -> Option<Self> {
        let centre = pixel_of(camera, frame.position)?;
        // The ring's edge is a point a ring-radius to the camera's right.
        let [f, u] = [camera.forward, camera.up];
        let right = [
            f[1] * u[2] - f[2] * u[1],
            f[2] * u[0] - f[0] * u[2],
            f[0] * u[1] - f[1] * u[0],
        ];
        let length = right.iter().map(|c| c * c).sum::<f32>().sqrt();
        if length.is_nan() || length <= 1e-6 {
            return None;
        }
        let at = |sign: f32| {
            pixel_of(
                camera,
                std::array::from_fn(|i| frame.position[i] + sign * RING * right[i] / length),
            )
        };
        let edge = at(1.0).or_else(|| at(-1.0))?;
        let radius = (edge[0] - centre[0])
            .abs()
            .max((edge[1] - centre[1]).abs())
            .max(2);
        Some(Marker {
            centre,
            radius,
            label: [centre[0], centre[1] - radius - GAP],
            colour,
        })
    }

    /// Draws the ring behind the HUD's reticle, text and panels: pixels
    /// that are already drawn are left alone, as the gizmo's are.
    pub fn paint(self, rgba: &mut [u8], size: (u32, u32)) {
        if size != (640, 480) || rgba.len() != 640 * 480 * 4 {
            return;
        }
        let [cx, cy] = self.centre;
        let colour = [self.colour[0], self.colour[1], self.colour[2], 255];
        let steps = (self.radius * 8).max(16);
        for n in 0..steps {
            let a = std::f32::consts::TAU * n as f32 / steps as f32;
            let x = cx + (a.cos() * self.radius as f32).round() as i32;
            let y = cy + (a.sin() * self.radius as f32).round() as i32;
            if (0..640).contains(&x) && (0..480).contains(&y) {
                let at = ((y * 640 + x) * 4) as usize;
                if rgba[at + 3] == 0 {
                    rgba[at..at + 4].copy_from_slice(&colour);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::gizmo::Axes;
    use super::*;

    fn camera() -> RenderCamera {
        RenderCamera {
            position: [0.; 3],
            forward: [1., 0., 0.],
            up: [0., 0., 1.],
            fov: 1.,
            projection_xy: None,
            viewport: [0, 0, 480, 640],
            near: 0.1,
            far: 90.,
        }
    }

    fn frame(x: f32) -> forge::Frame {
        forge::Frame {
            position: [x, 0., 0.],
            rotation: [0.; 3],
        }
    }

    #[test]
    fn the_marker_is_where_the_gizmo_is_for_the_same_frame() {
        for frame in [frame(4.), frame(9.)] {
            let axes = Axes::from_frame(frame, &camera()).unwrap();
            let marker = Marker::from_frame(frame, &camera(), [1, 2, 3]).unwrap();
            assert_eq!(marker.centre, axes.origin());
            assert_eq!(marker.centre, [320, 240], "straight ahead is the middle");
        }
        // Out of the view is out for both.
        assert!(Axes::from_frame(frame(-4.), &camera()).is_none());
        assert!(Marker::from_frame(frame(-4.), &camera(), [0; 3]).is_none());
    }

    #[test]
    fn the_ring_shrinks_with_distance_and_the_name_sits_above_it() {
        let near = Marker::from_frame(frame(3.), &camera(), [0; 3]).unwrap();
        let far = Marker::from_frame(frame(12.), &camera(), [0; 3]).unwrap();
        assert!(near.radius > far.radius, "{near:?} {far:?}");
        assert!(far.radius >= 2);
        assert_eq!(near.label, [320, 240 - near.radius - GAP]);
    }

    #[test]
    fn paint_draws_a_ring_in_the_colour_and_leaves_drawn_pixels() {
        let marker = Marker::from_frame(frame(4.), &camera(), [9, 8, 7]).unwrap();
        let mut rgba = vec![0u8; 640 * 480 * 4];
        let covered = (240 * 640 + 320) * 4;
        rgba[covered + 3] = 255;
        rgba[covered] = 1;
        marker.paint(&mut rgba, (640, 480));
        let lit = |x: i32, y: i32| rgba[((y * 640 + x) * 4) as usize + 3] == 255;
        assert!(lit(320 + marker.radius, 240), "the ring's right edge");
        assert!(lit(320, 240 + marker.radius), "the ring's bottom edge");
        assert!(
            !lit(320 + marker.radius * 2, 240),
            "nothing outside the ring"
        );
        let at = ((240 * 640 + 320 + marker.radius) * 4) as usize;
        assert_eq!(rgba[at..at + 4], [9, 8, 7, 255]);
        assert_eq!(rgba[covered], 1, "the centre was drawn already");
        // A canvas of another size is left alone.
        let mut other = vec![0u8; 16];
        marker.paint(&mut other, (2, 2));
        assert!(other.iter().all(|b| *b == 0));
    }
}
