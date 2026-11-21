//! The Objects tool's three local axes and the outline of the held
//! object's bounding sphere, projected with the same sealed world camera as
//! the scene. Only the compositor sees these pixels: the renderer has no
//! translucent preview of an object, so the outline shows how much of the
//! world the object will take.
#[cfg(any(feature = "desktop-runtime", test))]
use frame_scene::{RenderCamera, transform};

/// Projected axis endpoints in the HUD's 640 by 480 canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Axes {
    origin: [i32; 2],
    ends: [Option<[i32; 2]>; 3],
    /// The bounding sphere's outline: its centre and radius in pixels.
    ring: Option<([i32; 2], i32)>,
}

/// The largest outline drawn, in pixels; a bigger one is off the canvas.
#[cfg(any(feature = "desktop-runtime", test))]
const MAX_RING: f32 = 2048.0;

pub(super) fn project(frame: forge::Frame, extent: forge::tool::Extent) -> Option<Axes> {
    #[cfg(feature = "desktop-runtime")]
    {
        let scene = render_bridge::scene_step::last()?;
        let view = scene
            .views
            .iter()
            .find(|v| v.key.kind == frame_scene::ViewKind::Game && v.key.view == 0)?;
        Axes::from_frame(frame, extent, &view.camera)
    }
    #[cfg(not(feature = "desktop-runtime"))]
    {
        let _ = (frame, extent);
        None
    }
}

/// `p` in the canvas's pixels, unclipped, when it is in front of the camera.
#[cfg(any(feature = "desktop-runtime", test))]
fn transform_px(camera: &RenderCamera, p: [f32; 3]) -> Option<[f32; 2]> {
    let [top, left, bottom, right] = camera.viewport.map(f32::from);
    let [x, y, _, w] = transform(&camera.view_projection(), [p[0], p[1], p[2], 1.0]);
    (w > 0.0).then(|| {
        [
            left + (right - left) * (0.5 + x / w * 0.5),
            top + (bottom - top) * (0.5 - y / w * 0.5),
        ]
    })
}

impl Axes {
    #[cfg(any(feature = "desktop-runtime", test))]
    fn from_frame(
        frame: forge::Frame,
        extent: forge::tool::Extent,
        camera: &RenderCamera,
    ) -> Option<Self> {
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
        // The sphere's centre is its offset along the object's own axes;
        // its outline's radius is the radius seen at that depth.
        let centre: [f32; 3] = std::array::from_fn(|i| {
            frame.position[i]
                + extent.offset[0] * x[i] as f32
                + extent.offset[1] * y[i] as f32
                + extent.offset[2] * z[i] as f32
        });
        let ring = transform_px(camera, centre).and_then(|c| {
            let edge: [f32; 3] = std::array::from_fn(|i| centre[i] + extent.radius * camera.up[i]);
            let e = transform_px(camera, edge)?;
            let r = (e[0] - c[0]).hypot(e[1] - c[1]);
            (r.is_finite() && r >= 1.0 && r <= MAX_RING)
                .then(|| ([c[0].round() as i32, c[1].round() as i32], r.round() as i32))
        });
        Some(Self { origin, ends, ring })
    }

    /// Put the outline and the axes behind the HUD's reticle, text and
    /// panels. The same pixels are reused until an endpoint or the outline
    /// rounds to a different pixel.
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
        if let Some(([cx, cy], r)) = self.ring {
            // A dashed ring: every other arc of four pixels.
            let steps = (std::f32::consts::TAU * r as f32).ceil().max(16.0) as i32;
            for n in 0..steps {
                if (n / 4) % 2 == 1 {
                    continue;
                }
                let a = std::f32::consts::TAU * n as f32 / steps as f32;
                let (s, c) = a.sin_cos();
                let p = [
                    cx + (r as f32 * c).round() as i32,
                    cy + (r as f32 * s).round() as i32,
                ];
                pixel(p, [236, 240, 244, 200], rgba);
            }
        }
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
        let extent = forge::tool::Extent {
            radius: 1.,
            offset: [0.; 3],
        };
        let front = Axes::from_frame(frame, extent, &camera).unwrap();
        assert_eq!(front.origin, [320, 240]);
        assert_eq!(front.ends[0], Some(front.origin));
        assert!(front.ends[1].unwrap()[0] < 320);
        assert!(front.ends[2].unwrap()[1] < 240);
        frame.rotation[0] = std::f32::consts::FRAC_PI_2;
        let turned = Axes::from_frame(frame, extent, &camera).unwrap();
        assert_eq!(turned.ends[0], front.ends[1]);
        frame.position[0] = -4.;
        assert!(Axes::from_frame(frame, extent, &camera).is_none());
    }

    #[test]
    fn the_outline_is_the_spheres_size_at_its_depth_and_lifts_with_its_offset() {
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
        let frame = forge::Frame {
            position: [4., 0., 0.],
            rotation: [0.; 3],
        };
        let ring = |radius: f32, offset: [f32; 3], at: forge::Frame| {
            Axes::from_frame(at, forge::tool::Extent { radius, offset }, &camera)
                .unwrap()
                .ring
        };
        let (centre, near) = ring(1., [0.; 3], frame).unwrap();
        assert_eq!(centre, [320, 240]);
        assert!(near > 0);
        let far = forge::Frame {
            position: [8., 0., 0.],
            ..frame
        };
        let (_, small) = ring(1., [0.; 3], far).unwrap();
        assert!(
            (small - near / 2).abs() <= 1,
            "{small} at twice {near}'s depth"
        );
        let (lifted, _) = ring(1., [0., 0., 1.], frame).unwrap();
        assert!(
            lifted[1] < 240,
            "a centre above the origin is higher on screen"
        );
        assert_eq!(ring(0., [0.; 3], frame), None, "no size, no outline");
        let mut rgba = vec![0; 640 * 480 * 4];
        Axes::from_frame(
            frame,
            forge::tool::Extent {
                radius: 1.,
                offset: [0.; 3],
            },
            &camera,
        )
        .unwrap()
        .paint(&mut rgba, (640, 480));
        let at = (240 * 640 + 320 + near as usize) * 4;
        assert_eq!(rgba[at + 3], 200, "the ring's rightmost pixel");
    }
}
