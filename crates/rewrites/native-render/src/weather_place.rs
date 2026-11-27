//! The placement half of the weather plate function `0x0003a5c0`
//! (`0x0003a5c0..0x0003a7c2`): from the plate's two depth words and its
//! kind, the depth range the sheet covers and its fade, or a refusal
//! (`xor al,al` at `0x0003b880`: the plate is not drawn).
//!
//! The function is `stdcall` with nine arguments (`ret 24h`):
//! `(out, near, far, kind, a4, a5, a6, a7, result)`. The caller `0x00037a60`
//! passes the element's `+0x80` and `+0x84` floats as `near` and `far`
//! and `kind` 3. Here `kind` is the table at `0x0003b88c`; `scale` is the
//! float at `0x0048565c`. The three flags are the bytes the function tests
//! at `[esp+48h]`, `[esp+4Ch]`, `[esp+50h]`.
//!
//! What follows `0x0003a7c2` (the pixel shader definition at `0x00484f68`,
//! the `0x000222d0` render states, the shader select `0x0001cc30` and the
//! draw) is not ported here.

/// What the kind selects (`0x0003a61f..0x0003a651`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Kind {
    /// `al`: the near end may be rescaled by 256 when the range is small.
    pub rescale: bool,
    /// `bl`: the fade is derived from a negative near end.
    pub fade: bool,
    /// The cap on `1 / (far - near)` (`[esp+10h]`).
    pub cap: f32,
}

impl Kind {
    pub fn of(kind: u32) -> Self {
        let (rescale, fade, cap) = match kind {
            0 => (true, true, 512.0),
            1 => (false, true, 64.0),
            2 => (true, false, 128.0),
            3 => (false, false, 128.0),
            _ => (false, false, 1.0),
        };
        Self { rescale, fade, cap }
    }
}

/// The inputs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Input {
    pub kind: u32,
    pub near: f32,
    pub far: f32,
    /// `[esp+48h]`, `[esp+4Ch]`, `[esp+50h]` as booleans (a byte test).
    pub flags: [bool; 3],
    /// The float at `0x0048565c`.
    pub scale: f32,
}

/// The outputs the draw half reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range {
    /// `[esp+3Ch]` and `[esp+40h]` at `0x0003a7c2`.
    pub near: f32,
    pub far: f32,
    /// `[esp+10h]`: the kind's cap.
    pub cap: f32,
    /// `[esp+0Ch]`: the fade, 0 when the near end is not negative.
    pub fade: f32,
    /// `[esp+0Bh]`: the range was rescaled by 256.
    pub rescaled: bool,
}

/// `floor` then `_ftol` (`0x00321099`, `0x00322b0c`) and back to a float
/// scaled by 2^-16: the fixed point the function quantises to.
pub fn quantise(x: f32) -> f32 {
    let scaled = (f64::from(x) * 65536.0).floor();
    let i = if scaled >= -2147483648.0 && scaled < 2147483648.0 { scaled as i32 } else { i32::MIN };
    (i as f32) * (1.0 / 65536.0)
}

/// `comiss a, b; jbe` is taken when `a <= b` or either is NaN.
fn jbe(a: f32, b: f32) -> bool {
    !(a > b)
}

/// The placement, `None` for a refusal.
pub fn place(i: Input) -> Option<Range> {
    let Kind { rescale, fade: kind_fade, cap } = Kind::of(i.kind);
    let inv = 1.0 / i.scale;
    let mut near = i.near * inv;
    let mut far = i.far * inv;
    let mut rescaled = false;
    // `comiss 1.0, far*256; jb`: not taken when far*256 <= 1 (or NaN is
    // taken, so not here).
    let big = far * 256.0;
    let taken = 1.0 < big || big.is_nan();
    if !taken && rescale {
        near *= 256.0;
        far = big;
        rescaled = true;
    }
    let near_q = quantise(near);
    let far_q = quantise(far);
    // `comiss far_q, near_q; jbe fail`.
    if jbe(far_q, near_q) {
        return None;
    }
    let mut a = near_q;
    let b = far_q;
    let span = 1.0 / (b - a);
    let mut far_slot = span;
    // `comiss span, cap; jbe skip`.
    if !jbe(span, cap) {
        let inv_cap = 1.0 / cap;
        far_slot = cap;
        if rescaled {
            return None;
        }
        if i.flags[0] {
            a = (a + b) * 0.5 - inv_cap * 0.5;
        } else if i.flags[1] {
            a = b - inv_cap;
        } else if !i.flags[2] {
            return None;
        }
    }
    let mut fade = 0.0f32;
    if !jbe(0.0, a) {
        // `comiss 0, near; jbe skip` not taken: the near end is negative.
        if kind_fade {
            let t = 0.0 - far_slot * a;
            fade = if !jbe(0.0, t) {
                0.0
            } else if t > 1.0 {
                1.0
            } else {
                t
            };
        } else if !i.flags[1] || !i.flags[2] {
            return None;
        }
        a = 0.0;
    }
    Some(Range { near: a, far: far_slot, cap, fade, rescaled })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(kind: u32, near: f32, far: f32, flags: [bool; 3]) -> Input {
        Input { kind, near, far, flags, scale: 1.0 }
    }

    #[test]
    fn the_kind_table_follows_the_jump_table() {
        assert_eq!(Kind::of(0), Kind { rescale: true, fade: true, cap: 512.0 });
        assert_eq!(Kind::of(1), Kind { rescale: false, fade: true, cap: 64.0 });
        assert_eq!(Kind::of(2), Kind { rescale: true, fade: false, cap: 128.0 });
        assert_eq!(Kind::of(3), Kind { rescale: false, fade: false, cap: 128.0 });
        assert_eq!(Kind::of(9).cap, 1.0);
    }

    #[test]
    fn a_range_is_quantised_to_sixteen_bit_fractions() {
        assert_eq!(quantise(0.5), 0.5);
        assert_eq!(quantise(1.0 / 65536.0 * 1.5), 1.0 / 65536.0);
        assert_eq!(quantise(-1.0 / 65536.0 * 0.5), -1.0 / 65536.0);
    }

    #[test]
    fn an_empty_range_is_refused() {
        assert_eq!(place(input(3, 0.5, 0.5, [true; 3])), None);
        assert_eq!(place(input(3, 0.6, 0.5, [true; 3])), None);
    }

    #[test]
    fn a_wide_range_passes_through() {
        // 1 / (4 - 1) is under the cap of 128: nothing is moved.
        let r = place(input(3, 1.0, 4.0, [false; 3])).unwrap();
        assert_eq!((r.near, r.cap, r.fade, r.rescaled), (1.0, 128.0, 0.0, false));
        assert_eq!(r.far, 1.0 / 3.0);
    }

    #[test]
    fn a_narrow_range_needs_a_flag_to_be_widened() {
        // span 1000 over the cap of 128.
        let narrow = input(3, 1.0, 1.001, [false; 3]);
        assert_eq!(place(narrow), None);
        let r = place(Input { flags: [false, true, false], ..narrow }).unwrap();
        assert_eq!(r.far, 128.0);
        assert!((r.near - (quantise(1.001) - 1.0 / 128.0)).abs() < 1e-6);
    }

    #[test]
    fn a_negative_near_end_fades_in_kinds_with_a_fade() {
        let r = place(input(1, -0.25, 1.0, [false; 3])).unwrap();
        assert_eq!(r.near, 0.0);
        assert_eq!(r.fade, (0.0 - r.far * -0.25).clamp(0.0, 1.0));
        assert_eq!(place(input(3, -0.25, 1.0, [false; 3])), None);
        assert!(place(input(3, -0.25, 1.0, [false, true, true])).is_some());
    }
}
