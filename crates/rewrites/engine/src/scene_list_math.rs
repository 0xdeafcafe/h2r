//! The extended-precision tangents of the retail code: the stored
//! single-precision slopes from the non-rectangular branch of the light
//! shape builder, and the view projection's (`0x0002fb70`).

use x87::{F80, Mode, Pc, Rc};

pub fn cone_slopes(angle: f32, half: f32, aspect: f32) -> Option<(f32, f32)> {
    if !angle.is_finite() || !half.is_finite() || !aspect.is_finite() {
        return None;
    }
    let mode = Mode {
        pc: Pc::Double,
        rc: Rc::Near,
    };
    let extend = |value: f32| F80::from_f32_bits(value.to_bits());
    let narrow = |value: F80| f32::from_bits(value.to_f32_bits(mode).0);
    let tangent = x87::tan(x87::mul(extend(angle), extend(half), mode).0);
    if tangent.out_of_range {
        return None;
    }
    Some((
        narrow(tangent.value),
        narrow(x87::mul(tangent.value, extend(aspect), mode).0),
    ))
}

/// The view projection's tangent (`0x0002fb70`): `fld dword fov; fmul
/// dword half; fptan`, then the tangent stored to single, at double
/// precision and round to nearest. `None` for an operand `fptan` leaves
/// unreduced, or a NaN or infinite input.
pub fn view_tangent(fov: f32, half: f32) -> Option<f32> {
    if !fov.is_finite() || !half.is_finite() {
        return None;
    }
    let mode = Mode {
        pc: Pc::Double,
        rc: Rc::Near,
    };
    let extend = |value: f32| F80::from_f32_bits(value.to_bits());
    let tangent = x87::tan(x87::mul(extend(fov), extend(half), mode).0);
    if tangent.out_of_range {
        return None;
    }
    Some(f32::from_bits(tangent.value.to_f32_bits(mode).0))
}
