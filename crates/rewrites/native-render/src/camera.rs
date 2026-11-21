//! The render camera. The type and its view and projection are the
//! scene's ([`frame_scene::camera`]); the guest record and its read are
//! extraction's ([`scene_extract::layout::camera`]). [`GuestRead`] is
//! re-exported for the stages here that still read guest memory at their
//! hooks, until they move to `scene_extract` (slices 3 to 6).

pub use frame_scene::camera::*;
pub use scene_extract::layout::camera::{RENDER_CAMERA, read};
pub use scene_extract::{GuestRead, GuestWrite};

/// The camera used for both host visibility and rasterization. Scale the
/// complete clip-space X row, including any off-centre shift. Keep the
/// captured camera, vertical FOV, depth, and guest logical viewport intact.
pub fn display_camera(mut camera: RenderCamera, horizontal_factor: f32) -> RenderCamera {
    if horizontal_factor != 1.0 {
        let p = camera.projection();
        camera.projection_xy = Some([
            p[0][0] * horizontal_factor,
            p[1][1],
            p[2][0] * horizontal_factor,
            p[2][1],
        ]);
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
}
