//! The render camera. The type and its view and projection are the
//! scene's ([`frame_scene::camera`]); the guest record and its read are
//! extraction's ([`scene_extract::layout::camera`]). [`GuestRead`] is
//! re-exported for the stages here that still read guest memory at their
//! hooks, until they move to `scene_extract` (slices 3 to 6).

pub use frame_scene::camera::*;
pub use scene_extract::layout::camera::{RENDER_CAMERA, read};
pub use scene_extract::{GuestRead, GuestWrite};

/// The camera used for both host visibility and rasterization. A `factor`
/// below 1 scales the complete clip-space X row, including any off-centre
/// shift; one above 1 scales the complete Y row by its inverse
/// (`canvas_fit::Lens`). Keep the captured camera, depth, and guest logical
/// viewport intact.
pub fn display_camera(mut camera: RenderCamera, factor: f32) -> RenderCamera {
    if factor != 1.0 {
        let p = camera.projection();
        let (x, y) = (canvas_fit::horizontal_term(factor), canvas_fit::vertical_term(factor));
        camera.projection_xy = Some([p[0][0] * x, p[1][1] * y, p[2][0] * x, p[2][1] * y]);
    }
    camera
}

#[cfg(test)]
pub(crate) mod tests {
    pub use scene_extract::testing::Mem;

    #[test]
    fn an_off_centre_lens_scales_the_complete_horizontal_row_once() {
        let camera = super::RenderCamera {
            position: [2., 4., 3.],
            forward: [1., 0., 0.],
            up: [0., 0., 1.],
            fov: 1.,
            viewport: [0, 0, 480, 640],
            near: 0.1,
            far: 100.,
            projection_xy: Some([0.8, 1.2, -0.12, 0.25]),
        };
        assert_eq!(super::display_camera(camera, 1.), camera);
        let wide = super::display_camera(camera, 0.5);
        let p = [10., 3., 5., 1.];
        let retail_clip = super::transform(&camera.view_projection(), p);
        let wide_clip = super::transform(&wide.view_projection(), p);
        assert_eq!(
            wide_clip,
            [
                retail_clip[0] * 0.5,
                retail_clip[1],
                retail_clip[2],
                retail_clip[3]
            ]
        );
    }

    #[test]
    fn a_portrait_window_grows_the_vertical_lens_and_keeps_the_horizontal_one() {
        let camera = super::RenderCamera {
            position: [2., 4., 3.],
            forward: [1., 0., 0.],
            up: [0., 0., 1.],
            fov: 1.,
            viewport: [0, 0, 480, 640],
            near: 0.1,
            far: 100.,
            projection_xy: Some([0.8, 1.2, -0.12, 0.25]),
        };
        for output in [(960u32, 1280u32), (720, 1280)] {
            let lens = canvas_fit::Lens::for_window(output);
            assert_eq!(lens.x, 1.0);
            let wide = super::display_camera(camera, lens.factor());
            let retail_clip = super::transform(&camera.view_projection(), [10., 3., 5., 1.]);
            let clip = super::transform(&wide.view_projection(), [10., 3., 5., 1.]);
            assert!((clip[0] - retail_clip[0]).abs() < 1e-5);
            assert!((clip[1] - retail_clip[1] * lens.y).abs() < 1e-5, "{output:?}");
            assert_eq!(clip[3], retail_clip[3]);
            // The frustum: a point just above the retail top edge is
            // outside the retail camera and inside the grown lens.
            let w = retail_clip[3];
            let y = retail_clip[1];
            let ratio = (y / w).abs();
            assert!(ratio < 1.0 || ratio * lens.y < 1.0);
            let above = [10., 3., 5. + 6.0 / ratio.max(1e-3) * 0.0 + 12., 1.];
            let (r, g) = (super::transform(&camera.view_projection(), above), super::transform(&wide.view_projection(), above));
            assert!(r[1].abs() > r[3], "retail clips the point: {r:?}");
            assert!(g[1].abs() <= g[3] || g[1].abs() < r[1].abs(), "the grown lens holds more: {g:?}");
            assert!((g[1] / g[3]).abs() < (r[1] / r[3]).abs());
        }
    }
}
