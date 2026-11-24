//! Shadow-only instances from the live object pool. The root, sphere and
//! lighting sample reads follow the mask draw (0x32350) and 0x3dc90.
use super::{object_nodes::TAG_INSTANCES, objects};
use crate::GuestRead;
use frame_scene::{Camera, NearbyCaster};

const REACH: f32 = 8.0;
const LIMIT: usize = 32;
/// Default sample base used by the root branch at 0x0003dcb7.
pub const DEFAULT_LIGHTING: u32 = 0x004d_e2f4;

fn planes(camera: &Camera) -> [[f32; 4]; 6] {
    let m = camera.view_projection();
    std::array::from_fn(|i| {
        let mut p: [f32; 4] = std::array::from_fn(|j| match i {
            0 => m[j][3] + m[j][0],
            1 => m[j][3] - m[j][0],
            2 => m[j][3] + m[j][1],
            3 => m[j][3] - m[j][1],
            // Clip w is view depth. Using it directly avoids subtracting
            // nearly equal projection rows when far / near is large.
            4 => m[j][3] - if j == 3 { camera.near } else { 0.0 },
            _ => -m[j][3] + if j == 3 { camera.far } else { 0.0 },
        });
        let len = p[0].hypot(p[1]).hypot(p[2]);
        for x in &mut p {
            *x /= len;
        }
        p
    })
}

fn distance(p: [f32; 4], c: [f32; 3]) -> f32 {
    p[0] * c[0] + p[1] * c[1] + p[2] * c[2] + p[3]
}

fn candidate(
    m: &dyn GuestRead,
    header: u32,
    slot: u32,
    planes: &[[f32; 4]; 6],
) -> Option<NearbyCaster> {
    let word = m.read_u32(header)?;
    let salt = word as u16;
    if salt == 0 || (word >> 24) as u8 == objects::CREATURE {
        return None;
    }
    let handle = (u32::from(salt) << 16) | slot;
    let b = m.read_u32(header.checked_add(8)?)?;
    if b == 0 || b.checked_add(0x118).is_none() {
        return None;
    }
    let c = [
        m.read_f32(b + objects::BODY_SPHERE)?,
        m.read_f32(b + objects::BODY_SPHERE + 4)?,
        m.read_f32(b + objects::BODY_SPHERE + 8)?,
    ];
    let r = m.read_f32(b + objects::BODY_SPHERE_RADIUS)?;
    if !r.is_finite() || r <= 0.0 || !c.iter().all(|x| x.is_finite()) {
        return None;
    }
    let gaps = planes.map(|p| -distance(p, c) - r);
    if !gaps.iter().any(|&g| g > 0.0) || gaps.iter().any(|&g| g > REACH) {
        return None;
    }

    let flags = m.read_u32(b + 4)?;
    if flags & (1 << 16) != 0 || m.read_u32(b + 0x14)? != u32::MAX {
        return None;
    }
    let definition = m.read_u32(b)?;
    let tag = m
        .read_u32(TAG_INSTANCES)
        .filter(|&p| p != 0)?
        .checked_add((definition & 0xffff) * 16)?;
    let data = m.read_u32(tag.checked_add(8)?).filter(|&p| p != 0)?;
    let policy = m.read_u32(data.checked_add(0x18)?)? as u16;
    let level = (m.read_u32(b + 0xa8)? >> 16) as u8;
    if !(policy == 2 || (policy == 0 && level <= 1)) {
        return None;
    }
    let sample = if flags & (1 << 24) != 0 {
        m.read_u32(DEFAULT_LIGHTING)
            .filter(|&p| p != 0)?
            .checked_add(0x1c)?
    } else {
        let lighting = objects::object_lighting(m, handle, b)?;
        if m.read_u32(lighting.checked_sub(objects::CACHE_LIGHTING)?)? >> 24 == 0 {
            return None;
        }
        lighting.checked_add(0x10)?
    };
    let f = |offset| m.read_f32(sample.checked_add(offset)?);
    let dir = [f(0xc)?, f(0x10)?, f(0x14)?];
    let len = dir[0].hypot(dir[1]).hypot(dir[2]);
    let strength = f(0x1c)?;
    if !len.is_finite()
        || len == 0.0
        || !strength.is_finite()
        || strength <= frame_scene::ObjectShadow::DRAW_THRESHOLD
    {
        return None;
    }
    let towards_light = dir.map(|x| x / len);
    // Clip the swept sphere to all six half spaces over one common interval.
    let (mut enter, mut exit) = (0f32, REACH);
    for (p, g) in planes.iter().zip(gaps) {
        let speed = -distance([p[0], p[1], p[2], 0.0], towards_light);
        if speed > 0.0 {
            enter = enter.max(g / speed);
        } else if speed < 0.0 {
            exit = exit.min(g / speed);
        } else if g > 0.0 {
            return None;
        }
    }
    if enter > exit {
        return None;
    }
    Some(NearbyCaster {
        handle,
        definition,
        sphere: (c, r),
        nodes: objects::read_nodes(m, b)?,
        towards_light,
        strength,
    })
}

pub fn extract(m: &dyn GuestRead, camera: &Camera) -> Vec<NearbyCaster> {
    let Some(pool) = m.read_u32(objects::OBJECTS).filter(|&p| p != 0) else {
        return vec![];
    };
    let Some(count) = pool
        .checked_add(0x38)
        .and_then(|p| m.read_u32(p))
        .filter(|&n| n <= objects::MAX_SLOTS)
    else {
        return vec![];
    };
    let Some(headers) = pool
        .checked_add(0x44)
        .and_then(|p| m.read_u32(p))
        .filter(|&p| p != 0)
    else {
        return vec![];
    };
    let planes = planes(camera);
    (0..count)
        .filter_map(|i| candidate(m, headers.checked_add(i * 12)?, i, &planes))
        .take(LIMIT)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::camera;
    use crate::testing::{BODIES, CACHE_DATA, HEADERS, Mem, camera_record, object};

    #[test]
    fn seeded_pool_matches_the_axial_shadow_interval() {
        let mut m = Mem::default();
        camera_record(&mut m, 0x1000);
        let cam = camera::read(&m, 0x1000).unwrap();
        let h = object(&mut m, 0, 0, 1);
        m.w(TAG_INSTANCES, 0x2000);
        m.w(0x2008, 0x3000);
        m.w(0x3018, 2);
        m.w(BODIES + 4, 1 << 24);
        m.w(BODIES + 0x14, u32::MAX);
        m.w(BODIES + 0xa8, 0);
        m.w(DEFAULT_LIGHTING, 0x4000);
        let sample = 0x401c;
        m.f(sample + 0x10, 0.0);
        m.f(sample + 0x14, 0.0);
        m.f(sample + 0x1c, 0.75);
        let mut seed = 1301u32;
        for _ in 0..256 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let depth = (seed >> 8) as f32 / 16777216.0 * 24.0 - 16.0;
            let travel = if seed & 1 == 0 { 1.0 } else { -1.0 };
            m.f(sample + 0xc, -travel);
            m.f(BODIES + 0x30, cam.position[0] + depth);
            let expected =
                depth + 0.5 < cam.near && travel > 0.0 && depth + 0.5 + REACH >= cam.near;
            let got = extract(&m, &cam);
            assert_eq!(
                got.first().map(|c| c.handle),
                expected.then_some(h),
                "depth {depth}"
            );
        }
        // Valid cache branch, then stale owner, creature and malformed pose.
        m.f(BODIES + 0x30, -1.0);
        m.w(BODIES + 4, 0);
        let record = CACHE_DATA + 7 * objects::CACHE_RECORD_SIZE;
        m.w(record, 1 << 24);
        for (off, val) in [(0xc, -1.0), (0x10, 0.0), (0x14, 0.0), (0x1c, 0.75)] {
            m.f(record + 0x54 + off, val);
        }
        assert_eq!(extract(&m, &cam).len(), 1);
        m.w(record + 4, h ^ 1);
        assert!(extract(&m, &cam).is_empty());
        m.w(record + 4, h);
        m.w(HEADERS, u32::from(objects::CREATURE) << 24 | h >> 16);
        assert!(extract(&m, &cam).is_empty());
        m.w(HEADERS, h >> 16);
        m.f(BODIES + 0x200, f32::NAN);
        assert!(extract(&m, &cam).is_empty());
    }
}
