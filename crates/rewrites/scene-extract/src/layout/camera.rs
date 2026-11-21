//! The game's render camera record, read at a view's first layer draw.
//! The camera and its math are the scene's ([`frame_scene::camera`]).
//!
//! The record (`0x0002f800` fills it per view;
//! docs/graphics/occlusion-culling.md, "Camera and view"): position `+0`,
//! forward `+0xc`, up `+0x18`, vertical FOV `+0x28` (radians), viewport
//! `+0x30` (i16 top, left, bottom, right), near `+0x40`, far `+0x44`.

pub use crate::{GuestRead, GuestWrite};
pub use frame_scene::camera::*;

/// The render camera record's address (research point).
pub const RENDER_CAMERA: u32 = 0x004b_9da0;

/// Paired projection built by 0x0002fd90 from this camera. The diagonal
/// scales and off-centre terms include the game's own anamorphic and
/// cinematic settings. The display copy can change this lens after gather.
const RENDER_PROJECTION: u32 = 0x004b_9e14;

fn projection_xy(mem: &dyn GuestRead) -> Option<[f32; 4]> {
    let [x, y, dx, dy] = [0x78, 0x8c, 0x98, 0x9c].map(|offset| mem.read_f32(RENDER_PROJECTION + offset));
    let xy = [x?, y?, dx?, dy?];
    (xy.iter().all(|v| v.is_finite()) && xy[0] > 0.0 && xy[1] > 0.0).then_some(xy)
}

/// Reads the record at `addr`; `None` when unreadable or not a usable
/// camera (non-finite, zero axes, an empty viewport, `far <= near`).
pub fn read(mem: &dyn GuestRead, addr: u32) -> Option<RenderCamera> {
    let v3 = |o: u32| -> Option<[f32; 3]> {
        Some([mem.read_f32(addr + o)?, mem.read_f32(addr + o + 4)?, mem.read_f32(addr + o + 8)?])
    };
    let half = |o: u32| -> Option<i16> {
        let w = mem.read_u32((addr + o) & !3)?;
        Some(if (addr + o) & 2 == 0 { w as u16 as i16 } else { (w >> 16) as u16 as i16 })
    };
    let cam = RenderCamera {
        position: v3(0)?,
        forward: v3(0xc)?,
        up: v3(0x18)?,
        fov: mem.read_f32(addr + 0x28)?,
        projection_xy: if addr == RENDER_CAMERA { projection_xy(mem) } else { None },
        viewport: [half(0x30)?, half(0x32)?, half(0x34)?, half(0x36)?],
        near: mem.read_f32(addr + 0x40)?,
        far: mem.read_f32(addr + 0x44)?,
    };
    cam.is_usable().then_some(cam)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Mem, camera_record};

    #[test]
    fn the_record_reads_and_bad_records_do_not() {
        let mut m = Mem::default();
        camera_record(&mut m, RENDER_CAMERA);
        let c = read(&m, RENDER_CAMERA).unwrap();
        assert_eq!(c.position, [1.0, 2.0, 3.0]);
        assert_eq!(c.viewport, [0, 0, 480, 640]);
        assert_eq!((c.near, c.far), (0.0625, 1024.0));
        assert!((c.aspect() - 4.0 / 3.0).abs() < 1e-6);
        m.f(RENDER_CAMERA + 0x44, 0.01);
        assert_eq!(read(&m, RENDER_CAMERA), None, "far below near");
        camera_record(&mut m, RENDER_CAMERA);
        m.f(RENDER_CAMERA + 0xc, f32::NAN);
        assert_eq!(read(&m, RENDER_CAMERA), None, "NaN forward");
        camera_record(&mut m, RENDER_CAMERA);
        m.0.remove(&(RENDER_CAMERA + 0x44));
        assert_eq!(read(&m, RENDER_CAMERA), None, "unreadable");
    }

    #[test]
    fn captured_retail_four_by_three_projection_matches_the_computed_lens() {
        let mut m = Mem::default();
        camera_record(&mut m, RENDER_CAMERA);
        m.f(RENDER_CAMERA + 0x28, std::f32::consts::FRAC_PI_2);
        let computed = read(&m, RENDER_CAMERA).unwrap();
        assert_eq!(computed.projection_xy, None);

        // A centred 90-degree vertical lens has unit Y scale and 3/4
        // X scale on a 640x480 viewport. Use those analytical values
        // independently of the projection code.
        for (offset, value) in [(0x78, 0.75), (0x8c, 1.), (0x98, 0.), (0x9c, 0.)] {
            m.f(RENDER_PROJECTION + offset, value);
        }
        let captured = read(&m, RENDER_CAMERA).unwrap();
        assert_eq!(captured.projection_xy, Some([0.75, 1., 0., 0.]));
        for (actual, expected) in captured.view_projection().iter().flatten()
            .zip(computed.view_projection().iter().flatten())
        {
            assert!((actual - expected).abs() <= 1e-6, "{actual} versus {expected}");
        }
    }

    #[test]
    fn the_native_frustum_uses_the_rendered_lens_and_off_centre_terms() {
        let mut m = Mem::default();
        camera_record(&mut m, RENDER_CAMERA);
        for (offset, value) in [(0x78, 0.45), (0x8c, 1.6), (0x98, -0.1), (0x9c, 0.25)] {
            m.f(RENDER_PROJECTION + offset, value);
        }
        let c = read(&m, RENDER_CAMERA).unwrap();
        assert_eq!(c.viewport, [0, 0, 480, 640]);
        let p = c.projection();
        assert_eq!([p[0][0], p[1][1], p[2][0], p[2][1]], [0.45, 1.6, -0.1, 0.25]);
    }

    #[test]
    fn the_record_reads_as_the_view_the_projection_test_draws() {
        let mut m = Mem::default();
        camera_record(&mut m, RENDER_CAMERA);
        let c = read(&m, RENDER_CAMERA).unwrap();
        let p = transform(&c.view_projection(), [11.0, 2.0, 3.0, 1.0]);
        assert!((p[3] - 10.0).abs() < 1e-5 && p[0].abs() < 1e-5 && p[1].abs() < 1e-5, "{p:?}");
    }
}
