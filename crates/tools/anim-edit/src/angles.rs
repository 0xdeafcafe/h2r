//! Euler angles are an editor view of the stored quaternions.

use super::{dot, normalize};

/// Yaw, pitch and roll in degrees (Z, Y, X), solely for display.
pub fn rotation_to_angles(q: [f32; 4]) -> [f32; 3] {
    let [x, y, z, w] = q.map(f64::from);
    let pitch = (2.0 * (w * y - z * x)).clamp(-1.0, 1.0);
    let z_sin = 2.0 * (w * z + x * y);
    let z_cos = 1.0 - 2.0 * (y * y + z * z);
    let x_sin = 2.0 * (w * x + y * z);
    let x_cos = 1.0 - 2.0 * (x * x + y * y);
    let [yaw, pitch, roll] = if z_sin.hypot(z_cos) < 1e-6 {
        // At gimbal lock choose roll zero and recover the coupled yaw.
        [
            (2.0 * (w * z - x * y)).atan2(1.0 - 2.0 * (x * x + z * z)),
            pitch.signum() * std::f64::consts::FRAC_PI_2,
            0.0,
        ]
    } else {
        [z_sin.atan2(z_cos), pitch.asin(), x_sin.atan2(x_cos)]
    };
    [yaw, pitch, roll].map(|v| v.to_degrees() as f32)
}

/// Display the equivalent Euler branch nearest the previous point.
pub fn angles_near(q: [f32; 4], previous: [f32; 3]) -> [f32; 3] {
    let [yaw, pitch, roll] = rotation_to_angles(q);
    let unwrap = |v: [f32; 3]| {
        std::array::from_fn(|i| {
            previous[i] + (v[i] - previous[i] + 180.0).rem_euclid(360.0) - 180.0
        })
    };
    let normal = unwrap([yaw, pitch, roll]);
    let alternate = unwrap([yaw + 180.0, 180.0 - pitch, roll + 180.0]);
    let error = |a: [f32; 3]| -> f32 {
        a.into_iter()
            .zip(previous)
            .map(|(a, b)| (a - b).powi(2))
            .sum()
    };
    if error(alternate) < error(normal) {
        alternate
    } else {
        normal
    }
}

/// Convert an explicitly edited angle to the stored quaternion.
/// Viewing a key never calls this or rewrites it.
pub fn angles_to_rotation(angles: [f32; 3]) -> [f32; 4] {
    let [(sz, cz), (sy, cy), (sx, cx)] = angles.map(|a| (0.5 * a.to_radians()).sin_cos());
    [
        cz * cy * sx - sz * sy * cx,
        cz * sy * cx + sz * cy * sx,
        sz * cy * cx - cz * sy * sx,
        cz * cy * cx + sz * sy * sx,
    ]
}

/// Convert an angle-handle slope (degrees per frame) to a quaternion derivative.
pub fn tangent_from_angles(q: [f32; 4], angles: [f32; 3], slope: [f32; 3]) -> [f32; 4] {
    let half_radians = slope.map(|s| 0.5 * s.to_radians());
    let mut derivative = [0.0; 4];
    for axis in 0..3 {
        let mut offset = angles;
        // For q(a) = c*cos(a/2) + s*sin(a/2), dq/da = q(a+π)/2.
        offset[axis] += 180.0;
        let shifted = angles_to_rotation(offset);
        for i in 0..4 {
            derivative[i] += shifted[i] * half_radians[axis];
        }
    }
    if dot(q, angles_to_rotation(angles)) < 0.0 {
        derivative.map(|v| -v)
    } else {
        derivative
    }
}

/// The angle-view slope of a stored quaternion derivative near a key.
pub fn tangent_to_angles(q: [f32; 4], angles: [f32; 3], tangent: [f32; 4]) -> [f32; 3] {
    let magnitude = tangent.into_iter().map(f32::abs).fold(0.0, f32::max);
    if magnitude == 0.0 {
        return [0.0; 3];
    }
    let dt = 0.01 / magnitude.max(1.0);
    let Some(after) = normalize(std::array::from_fn(|i| q[i] + dt * tangent[i])) else {
        return [0.0; 3];
    };
    let after = angles_near(after, angles);
    std::array::from_fn(|i| (after[i] - angles[i]) / dt)
}
