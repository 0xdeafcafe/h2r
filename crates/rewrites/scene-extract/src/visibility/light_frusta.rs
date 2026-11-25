//! Flood lenses for the posed light volume passed to `0x000c1d90`.
//! Each lens maps the initial four tangent-space corners onto the flood's
//! screen rectangle. Projected lights also supply a world-space clip plane.

use super::Decline;
use super::flood::Lens;
use super::guest::{FRUSTUM_NEGATIVE_UNIT, FRUSTUM_UNIT, NORMALIZE_EPSILON};
use super::read::{at, f32_at, ptr_at, u16_at, vec3_at};
use super::view::lens::HALF;
use crate::GuestRead;

const CUBE_REFERENCE: u32 = 0x0046_87a8;
const CUBE_ALTERNATE: u32 = 0x0046_87ac;
/// The clip tolerance argument pushed by the portal projector (0x001735a8).
pub(super) const PORTAL_CLIP: u32 = 0x0017_35a9;

type LightLens = (Lens, Option<[f32; 4]>);

fn cross([x, y, z]: [f32; 3], [u, v, w]: [f32; 3]) -> [f32; 3] {
    [y * w - z * v, z * u - x * w, x * v - y * u]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[2] * b[2] + a[1] * b[1]) + a[0] * b[0]
}

/// `pose` is the frame at L+0x84 and `volume` the posed 31-word record.
/// A zero apex in the projected branch yields non-finite rectangle edges.
pub(super) fn frusta(
    mem: &dyn GuestRead,
    pose: u32,
    volume: u32,
    count: u16,
) -> Result<Vec<LightLens>, Decline> {
    let field = |base, offset| at(base, offset).ok_or(Decline::Layout);
    let word = |p| f32_at(mem, p).ok_or(Decline::Unread);
    let vec = |p| vec3_at(mem, p).ok_or(Decline::Unread);
    let kind = u16_at(mem, volume).ok_or(Decline::Unread)?;
    if count != if kind == 0 { 6 } else { 1 } {
        return Err(Decline::Frusta);
    }
    let position = vec(field(volume, 0x2c)?)?;
    let near = word(PORTAL_CLIP)?;
    let one = word(FRUSTUM_UNIT)?;
    let minus = word(FRUSTUM_NEGATIVE_UNIT)?;
    if kind == 0 {
        let far = word(field(volume, 0x44)?)?;
        let epsilon = word(NORMALIZE_EPSILON)?;
        let reference = |addr| vec(ptr_at(mem, addr).ok_or(Decline::Unread)?);
        let primary = reference(CUBE_REFERENCE)?;
        let alternate = reference(CUBE_ALTERNATE)?;
        let mut lenses = Vec::with_capacity(6);
        // Bottom, four sides around the equator, top.
        for face in 0..6 {
            let axis = if face == 0 || face == 5 {
                2
            } else {
                (face - 1) % 2
            };
            let mut forward = [0.0; 3];
            forward[axis] = if matches!(face, 1 | 2 | 5) {
                one
            } else {
                minus
            };
            let back = forward.map(|v| -v);
            let mut up = cross(back, if axis == 0 { alternate } else { primary });
            let [x, y, z] = up.map(f64::from);
            let length = (x * x + y * y + z * z).sqrt();
            if length.abs() >= f64::from(epsilon) {
                let inverse = one / length as f32;
                up = up.map(|v| v * inverse);
            }
            let right = cross(up, back);
            let lens = Lens::from_tangents(
                position,
                [right, up, forward],
                [minus, one, minus, one],
                near,
                far,
            )
            .ok_or(Decline::Camera)?;
            lenses.push((lens, None));
        }
        return Ok(lenses);
    }
    let right = vec(field(volume, 0x1c)?)?;
    let up = vec(field(pose, 0x34)?)?;
    let forward = vec(field(pose, 0x28)?)?;
    let axis = vec(field(volume, 0x3c)?)?;
    let apex = word(field(volume, 0x28)?)?;
    let far = word(field(volume, 0x64)?)? + apex;
    let inv_depth = minus / dot(forward.map(|v| -v), axis);
    let x = dot(right, axis) * inv_depth;
    let y = dot(up, axis) * inv_depth;
    let half = word(HALF)?;
    let inverse = one / apex;
    let width = word(field(volume, 0x58)?)? * inverse * half;
    let height = word(field(volume, 0x5c)?)? * inverse * half;
    let lens = Lens::from_tangents(
        position,
        [right, up, forward],
        [x - width, x + width, y - height, y + height],
        near,
        far,
    )
    .ok_or(Decline::Camera)?;
    let point = [0, 1, 2].map(|i| forward[i] * apex + position[i]);
    let clip = (apex > 0.0).then(|| [forward[0], forward[1], forward[2], dot(forward, point)]);
    Ok(vec![(lens, clip)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Mem;

    const POSE: u32 = 0x2000;
    const VOLUME: u32 = 0x3000;

    fn vector(m: &mut Mem, addr: u32, v: [f32; 3]) {
        for (i, value) in v.into_iter().enumerate() {
            m.f(addr + 4 * i as u32, value);
        }
    }

    fn common(kind: u32) -> Mem {
        let mut m = Mem::default();
        m.w(VOLUME, kind);
        vector(&mut m, VOLUME + 0x2c, [2.0, -3.0, 4.0]);
        let tolerance = 0.125f32.to_bits();
        m.w(PORTAL_CLIP & !3, tolerance << 8);
        m.w((PORTAL_CLIP & !3) + 4, tolerance >> 24);
        m.f(FRUSTUM_UNIT, 1.0);
        m.f(FRUSTUM_NEGATIVE_UNIT, -1.0);
        m
    }

    #[test]
    fn projected_light_offsets_the_window_and_clips_at_its_apex_distance() {
        let mut m = common(2);
        vector(&mut m, VOLUME + 0x1c, [0.0, -1.0, 0.0]);
        vector(&mut m, POSE + 0x28, [1.0, 0.0, 0.0]);
        vector(&mut m, POSE + 0x34, [0.0, 0.0, 1.0]);
        vector(&mut m, VOLUME + 0x3c, [1.0, -0.5, 0.25]);
        m.f(VOLUME + 0x28, 2.0);
        m.f(VOLUME + 0x58, 4.0);
        m.f(VOLUME + 0x5c, 2.0);
        m.f(VOLUME + 0x64, 7.0);
        m.f(HALF, 0.5);
        let (lens, clip) = frusta(&m, POSE, VOLUME, 1).unwrap()[0];
        assert_eq!(lens.tangent([-1.0, -1.0]), [-0.5, -0.25]);
        assert_eq!(lens.tangent([1.0, 1.0]), [1.5, 0.75]);
        assert_eq!((lens.near, lens.far), (0.125, 9.0));
        assert_eq!(clip, Some([1.0, 0.0, 0.0, 4.0]));
        m.f(VOLUME + 0x28, 0.0);
        assert_eq!(frusta(&m, POSE, VOLUME, 1), Err(Decline::Camera));
    }

    #[test]
    fn cube_faces_cover_each_axis_at_the_volume_radius() {
        let mut m = common(0);
        m.f(VOLUME + 0x44, 9.0);
        m.w(CUBE_REFERENCE, 0x4400);
        m.w(CUBE_ALTERNATE, 0x4500);
        vector(&mut m, 0x4400, [1.0, 0.0, 0.0]);
        vector(&mut m, 0x4500, [0.0, 1.0, 0.0]);
        m.f(NORMALIZE_EPSILON, 0.001);
        let faces = frusta(&m, POSE, VOLUME, 6).unwrap();
        for axis in 0..3 {
            for sign in [-1.0, 1.0] {
                let mut point = [2.0, -3.0, 4.0];
                point[axis] += sign * 9.0;
                assert_eq!(
                    faces
                        .iter()
                        .filter(|(lens, clip)| {
                            let p = lens.to_view(point);
                            p == [0.0, 0.0, lens.far] && clip.is_none()
                        })
                        .count(),
                    1
                );
            }
        }
        assert_eq!(frusta(&m, POSE, VOLUME, 1), Err(Decline::Frusta));
    }
}
