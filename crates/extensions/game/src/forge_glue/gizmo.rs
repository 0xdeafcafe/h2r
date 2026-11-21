//! The Objects tool's three local axes, projected with the same sealed world
//! camera as the scene. Only the compositor sees these pixels.
#[cfg(any(feature = "desktop-runtime", test))]
use frame_scene::{RenderCamera, transform};

/// Projected axis endpoints in the HUD's 640 by 480 canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Axes {
    origin: [i32; 2],
    ends: [Option<[i32; 2]>; 3],
}

pub(super) fn project(frame: forge::Frame) -> Option<Axes> {
    #[cfg(feature = "desktop-runtime")]
    {
        let scene = render_bridge::scene_step::last()?;
        let view = scene
            .views
            .iter()
            .find(|v| v.key.kind == frame_scene::ViewKind::Game && v.key.view == 0)?;
        Axes::from_frame(frame, &view.camera)
    }
    #[cfg(not(feature = "desktop-runtime"))]
    {
        let _ = frame;
        None
    }
}

impl Axes {
    #[cfg(any(feature = "desktop-runtime", test))]
    fn from_frame(frame: forge::Frame, camera: &RenderCamera) -> Option<Self> {
        if !camera.is_usable() {
            return None;
        }
        let [top, left, bottom, right] = camera.viewport.map(f32::from);
        if top < 0.0 || left < 0.0 || bottom > 480.0 || right > 640.0 {
            return None;
        }
        let vp = camera.view_projection();
        let point = |p: [f32; 3]| {
            let [x, y, z, w] = transform(&vp, [p[0], p[1], p[2], 1.0]);
            if !(w > 0.0 && (0.0..=w).contains(&z)) {
                return None;
            }
            let [x, y] = [x / w, y / w];
            if ![x, y].into_iter().all(|v| (-1.0..=1.0).contains(&v)) {
                return None;
            }
            Some([
                (left + (right - left) * (0.5 + x * 0.5)).round() as i32,
                (top + (bottom - top) * (0.5 - y * 0.5)).round() as i32,
            ])
        };
        let origin = point(frame.position)?;
        let (x, z) = live_telemetry::rotation_axes(frame.rotation.map(f64::from));
        let y = [
            z[1] * x[2] - z[2] * x[1],
            z[2] * x[0] - z[0] * x[2],
            z[0] * x[1] - z[1] * x[0],
        ];
        // Near the camera, limit foreshortening; at normal distances the
        // half-unit arms give the monitor a scale reference on the surface.
        let length = (camera.depth_of(frame.position) * 0.14).min(0.5);
        let ends = [x, y, z].map(|axis| {
            point(std::array::from_fn(|i| {
                frame.position[i] + length * axis[i] as f32
            }))
        });
        Some(Self { origin, ends })
    }

    /// Put the axes behind the HUD's reticle, text and panels. The same
    /// pixels are reused until an endpoint rounds to a different pixel.
    pub fn paint(self, rgba: &mut [u8], size: (u32, u32)) {
        if size != (640, 480) || rgba.len() != 640 * 480 * 4 {
            return;
        }
        let pixel = |[x, y]: [i32; 2], colour: [u8; 4], rgba: &mut [u8]| {
            if (0..640).contains(&x) && (0..480).contains(&y) {
                let at = ((y * 640 + x) * 4) as usize;
                if rgba[at + 3] == 0 {
                    rgba[at..at + 4].copy_from_slice(&colour);
                }
            }
        };
        for (end, rgb) in
            self.ends
                .into_iter()
                .zip([[255, 111, 92], [125, 228, 136], [94, 174, 255]])
        {
            let Some([x, y]) = end else { continue };
            let [x0, y0] = self.origin;
            let (dx, dy) = (x - x0, y - y0);
            let steps = dx.abs().max(dy.abs()).max(1);
            let colour = [rgb[0], rgb[1], rgb[2], 255];
            for n in 0..=steps {
                let p = [x0 + dx * n / steps, y0 + dy * n / steps];
                pixel(p, colour, rgba);
                if dx.abs() > dy.abs() {
                    pixel([p[0], p[1] + 1], colour, rgba);
                } else {
                    pixel([p[0] + 1, p[1]], colour, rgba);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_yaw_quarter_turn_exchanges_the_forward_and_lateral_arms() {
        let camera = RenderCamera {
            position: [0.; 3],
            forward: [1., 0., 0.],
            up: [0., 0., 1.],
            fov: 1.,
            projection_xy: None,
            viewport: [0, 0, 480, 640],
            near: 0.1,
            far: 90.,
        };
        let mut frame = forge::Frame {
            position: [4., 0., 0.],
            rotation: [0.; 3],
        };
        let front = Axes::from_frame(frame, &camera).unwrap();
        assert_eq!(front.origin, [320, 240]);
        assert_eq!(front.ends[0], Some(front.origin));
        assert!(front.ends[1].unwrap()[0] < 320);
        assert!(front.ends[2].unwrap()[1] < 240);
        frame.rotation[0] = std::f32::consts::FRAC_PI_2;
        let turned = Axes::from_frame(frame, &camera).unwrap();
        assert_eq!(turned.ends[0], front.ends[1]);
        frame.position[0] = -4.;
        assert!(Axes::from_frame(frame, &camera).is_none());
    }
}
