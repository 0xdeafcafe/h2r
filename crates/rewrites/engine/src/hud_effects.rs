//! `0x0022bcaf`, the HUD widget effects, instruction for instruction: a
//! widget's first effects element turns, moves and scales the basis its
//! art is laid out on (docs/graphics/native-hud-overlay.md, "The walk in
//! Rust").
//!
//! (Code, `h2 disasm 0x22bcaf 150`.) EAX points at the widget's effects
//! block (count, then address); nothing happens while the count is 0.
//! The first element's flags (`+0`) pick the parts, in this order:
//!
//! | Bit | Part | Input names | Functions |
//! |---|---|---|---|
//! | 2 | turn both axes by an angle (x87 `fsin`, `fcos`) | `+0x2c` | `+0x38` |
//! | 4 | move the origin along each axis | `+0x40`, `+0x54` | `+0x4c`, `+0x60` |
//! | 1 | scale each axis | `+0x04`, `+0x18` | `+0x10`, `+0x24` |
//!
//! Each value is the function evaluator `0x0013b390` at the HUD's input
//! for the name (the callback `0x0022ace7`) and range input 1.0, then its
//! output range (`0x0013bb40`). The SSE arithmetic keeps the original's
//! operand order and the turn's x87 half runs on the x87 model, so a
//! basis here has the bits the guest's has.

use crate::function_eval;
use crate::light_constants::{Error, Sse, X87};
use guest_layout::memory::GuestMemory;
use crate::screen_effects::ranged;

use x87::abi::arith_op;

/// 1.0 as `fld1` stores it.
const ONE: u32 = 0x3f80_0000;

/// The basis a widget is laid out on, as f32 bits: the point its
/// registration lands on and the two axes its art spans.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Basis {
    pub origin: [u32; 2],
    pub x: [u32; 2],
    pub y: [u32; 2],
}

impl Basis {
    /// The basis the widget draws set before the call: axes (1, 0) and
    /// (0, 1) at `origin`.
    pub fn at(origin: [f32; 2]) -> Self {
        Basis { origin: origin.map(f32::to_bits), x: [ONE, 0], y: [0, ONE] }
    }
}

/// One value: the function at `element + function` over the input named
/// at `element + name`. `None`: the input is one the callback computes by
/// a call, which `input` does not read.
fn value(
    mem: &dyn GuestMemory,
    sse: &mut Sse,
    control: u16,
    input: &dyn Fn(u32) -> Option<f32>,
    element: u32,
    name: u32,
    function: u32,
) -> Result<Option<u32>, Error> {
    let Some(x) = input(mem.u32(element.wrapping_add(name))?) else { return Ok(None) };
    let reference = element.wrapping_add(function);
    let v = function_eval::evaluate(mem, sse, control, reference, x.to_bits(), ONE)?;
    // `0x0013bb40` reads the blob with no check: the guest faults on none.
    let data = mem.u32(reference.wrapping_add(4))?;
    if data == 0 {
        return Err(Error::Data);
    }
    ranged(mem, sse, data, v).map(Some)
}

/// `0x0022bcaf` on `basis` for the effects block `(count, address)`,
/// under the caller's MXCSR and x87 control word. `input` is the HUD's
/// function input callback (`0x0022ace7`) by name. `Ok(false)`: an input
/// the callback computes by a call, and `basis` is not what the guest's
/// would be.
pub fn apply(
    mem: &dyn GuestMemory,
    mxcsr: u32,
    control: u16,
    block: (u32, u32),
    input: &dyn Fn(u32) -> Option<f32>,
    basis: &mut Basis,
) -> Result<bool, Error> {
    let (count, element) = block;
    if count == 0 {
        return Ok(true);
    }
    let mut sse = Sse { mxcsr };
    let flags = mem.u8(element)?;
    if flags & 2 != 0 {
        let Some(angle) = value(mem, &mut sse, control, input, element, 0x2c, 0x38)? else { return Ok(false) };
        let mut x = X87::new(control);
        x.fld(angle)?;
        x.fsin()?;
        let sin = x.fst_m32()?;
        x.fld(angle)?;
        x.fcos()?;
        let cos = x.fst_m32()?;
        // st0 the cosine, st1 the sine.
        for axis in [&mut basis.x, &mut basis.y] {
            let by_sin = sse.mul(axis[0], sin)?;
            let by_cos = sse.mul(axis[1], cos)?;
            x.fld_st(0)?;
            x.fmul_m(axis[0])?;
            let second = sse.add(by_cos, by_sin)?;
            x.fld_st(2)?;
            x.fmul_m(axis[1])?;
            axis[1] = second;
            x.arith_reg(arith_op::SUB, 1, 0, true)?;
            axis[0] = x.fstp_m32()?;
        }
    }
    if flags & 4 != 0 {
        let Some(along_x) = value(mem, &mut sse, control, input, element, 0x40, 0x4c)? else { return Ok(false) };
        let Some(along_y) = value(mem, &mut sse, control, input, element, 0x54, 0x60)? else { return Ok(false) };
        let step = sse.mul(basis.x[0], along_x)?;
        let px = sse.add(step, basis.origin[0])?;
        let step = sse.mul(basis.x[1], along_x)?;
        let py = sse.add(basis.origin[1], step)?;
        let down = sse.mul(basis.y[1], along_y)?;
        let across = sse.mul(basis.y[0], along_y)?;
        basis.origin = [sse.add(across, px)?, sse.add(py, down)?];
    }
    if flags & 1 != 0 {
        let Some(sx) = value(mem, &mut sse, control, input, element, 0x04, 0x10)? else { return Ok(false) };
        let Some(sy) = value(mem, &mut sse, control, input, element, 0x18, 0x24)? else { return Ok(false) };
        basis.x = [sse.mul(basis.x[0], sx)?, sse.mul(basis.x[1], sx)?];
        basis.y = [sse.mul(basis.y[0], sy)?, sse.mul(basis.y[1], sy)?];
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    const ELEMENT: u32 = 0x4000;
    /// The x87 control word and MXCSR the game runs under.
    const CONTROL: u16 = 0x027f;
    const MXCSR: u32 = 0x1f80;

    /// An element whose every function is the identity taken raw (flags
    /// `0xf0`: no output range), so each value is its input clamped to
    /// 0..1; the names are their own offsets.
    fn element(flags: u8) -> FakeGuest {
        let mut m = FakeGuest::new();
        m.put_f32(0x0045_dbc0, 1.0);
        m.put_u8(ELEMENT, flags);
        for (n, (name, function)) in [(0x04, 0x10), (0x18, 0x24), (0x2c, 0x38), (0x40, 0x4c), (0x54, 0x60)].into_iter().enumerate() {
            let blob = 0x5000 + 0x100 * n as u32;
            m.put_u32(ELEMENT + name, name).put_u32(ELEMENT + function, 0x20).put_u32(ELEMENT + function + 4, blob);
            m.put_u8(blob, 0).put_u8(blob + 1, 0xf0);
        }
        m
    }

    fn floats(b: &Basis) -> [[f32; 2]; 3] {
        [b.origin, b.x, b.y].map(|v| v.map(f32::from_bits))
    }

    #[test]
    fn a_widget_with_no_effects_keeps_its_basis() {
        let m = FakeGuest::new();
        let mut basis = Basis::at([48.0, 444.0]);
        assert_eq!(apply(&m, MXCSR, CONTROL, (0, 0), &|_| Some(1.0), &mut basis), Ok(true));
        assert_eq!(basis, Basis::at([48.0, 444.0]));
    }

    #[test]
    fn the_scale_multiplies_each_axis_by_its_own_function() {
        let m = element(1);
        let mut basis = Basis::at([10.0, 20.0]);
        let input = |name: u32| Some(if name == 0x04 { 0.5 } else { 0.25 });
        assert_eq!(apply(&m, MXCSR, CONTROL, (1, ELEMENT), &input, &mut basis), Ok(true));
        assert_eq!(floats(&basis), [[10.0, 20.0], [0.5, 0.0], [0.0, 0.25]]);
    }

    #[test]
    fn the_offset_moves_the_origin_along_both_axes() {
        let m = element(4);
        let mut basis = Basis::at([10.0, 20.0]);
        basis.x = [2.0f32.to_bits(), 0];
        basis.y = [0, 4.0f32.to_bits()];
        let input = |name: u32| Some(if name == 0x40 { 0.5 } else { 0.25 });
        assert_eq!(apply(&m, MXCSR, CONTROL, (1, ELEMENT), &input, &mut basis), Ok(true));
        assert_eq!(floats(&basis)[0], [11.0, 21.0]);
    }

    #[test]
    fn the_turn_rotates_both_axes_before_the_scale() {
        let m = element(2 | 1);
        let mut basis = Basis::at([0.0, 0.0]);
        // An angle of 1 radian (the identity clamps at 1), then a half scale.
        let input = |name: u32| Some(if name == 0x2c { 1.0 } else { 0.5 });
        assert_eq!(apply(&m, MXCSR, CONTROL, (1, ELEMENT), &input, &mut basis), Ok(true));
        let [_, x, y] = floats(&basis);
        let (s, c) = (1.0f64.sin() as f32, 1.0f64.cos() as f32);
        for (got, want) in [(x[0], c * 0.5), (x[1], s * 0.5), (y[0], -s * 0.5), (y[1], c * 0.5)] {
            assert!((got - want).abs() < 1e-6, "{got} {want}");
        }
    }

    #[test]
    fn an_input_the_callback_computes_is_not_applied() {
        let m = element(1);
        let mut basis = Basis::at([1.0, 2.0]);
        assert_eq!(apply(&m, MXCSR, CONTROL, (1, ELEMENT), &|_| None, &mut basis), Ok(false));
    }

    #[test]
    fn a_function_with_no_blob_is_an_error_as_the_guest_faults() {
        let mut m = element(1);
        m.put_u32(ELEMENT + 0x10 + 4, 0);
        let mut basis = Basis::at([1.0, 2.0]);
        assert_eq!(apply(&m, MXCSR, CONTROL, (1, ELEMENT), &|_| Some(1.0), &mut basis), Err(Error::Data));
    }
}
