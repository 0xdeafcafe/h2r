//! The extended-precision tangent and the stored single-precision slopes
//! from the non-rectangular branch of the retail light shape builder.

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
