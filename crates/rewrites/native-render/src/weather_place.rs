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
    let i = if (-2147483648.0..2147483648.0).contains(&scaled) { scaled as i32 } else { i32::MIN };
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

/// One device operation of the draw half, in the order the function
/// issues it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// `0x00014bc0(al = 1, target, 0)` with the target id read from
    /// `[0x004858b8]`, then `0x00014f60`.
    Bind(u16),
    /// The inlined pushbuffer and shadow-state writes at `0x0003a7e8` to
    /// `0x0003a940`, which the guest device owns.
    DeviceReset,
    /// `0x000222d0` with `edi` the value and `esi` the render word index.
    State(u8, u32),
    /// `0x003f59b0(0)`, after which `[0x004b8450]` is 0.
    PushState0,
    /// `0x0001cc30` with `edx` the engine vertex shader slot.
    VertexShader(u32),
    /// `0x0001c710(0x0051f0f0)`.
    PixelProgram(u32),
    /// `0x0001ccf0(0x00484f68)`: apply the 60-word definition.
    Define,
}

/// The bytes of the view's arguments `0x131ed0` packs: `[edi]` and the
/// float triples at `+4`, `+0x10`, `+0x1c`, `+0x28` of argument 0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colours {
    pub flag: bool,
    pub rgb: [[f32; 3]; 4],
}

/// `0x00131ed0`: three floats times 255, each rounded (`fistp`, under the
/// guest's control word, here to nearest even) and masked to a byte, as
/// `0x00rrggbb`.
pub fn pack_colour(c: [f32; 3]) -> u32 {
    let b = |x: f32| (f64::from(x) * 255.0).round_ties_even() as i64 as u32 & 0xff;
    b(c[0]) << 16 | b(c[1]) << 8 | b(c[2])
}

/// `floor` then `_ftol`, clamped to a byte (`0x0002b480`, `0x00322b0c`).
fn clamp_byte(x: f32) -> u32 {
    let f = f64::from(x).floor();
    let i = if (-2147483648.0..2147483648.0).contains(&f) { f as i32 } else { i32::MIN };
    i.clamp(0, 0xff) as u32
}

/// The globals the function reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Globals {
    /// `[0x0048565c]`.
    pub scale: f32,
    /// `[0x00485658]`.
    pub limit: f32,
    /// `[0x004858b8]`.
    pub target: u16,
}

/// What the function leaves: the operations, the 60-word definition at
/// `0x00484f68` as applied, and the float written through argument 8.
#[derive(Clone, Debug, PartialEq)]
pub struct Plate {
    pub ops: Vec<Op>,
    pub define: [u32; 60],
    /// The words of `define` the function writes; the others keep what the
    /// buffer held before (the last definition built there).
    pub written: [bool; 60],
    pub result: Option<f32>,
}

const DEFINE: u32 = 0x0048_4f68;

/// The definition as it is built: its words and which of them are set.
struct Def {
    w: [u32; 60],
    set: [bool; 60],
}

fn word(d: &mut Def, addr: u32, v: u32) {
    let i = ((addr - DEFINE) / 4) as usize;
    d.w[i] = v;
    d.set[i] = true;
}

/// `0x0003a7cf..0x0003a984`: the first use of the device in the pass
/// (argument 7 zero).
fn reset(ops: &mut Vec<Op>, target: u16) {
    ops.extend([
        Op::Bind(target),
        Op::DeviceReset,
        Op::State(0x39, 0x207),
        Op::PushState0,
        Op::State(0x52, 0x10),
        Op::VertexShader(0xc),
        Op::PixelProgram(0x0051_f0f0),
    ]);
}

/// The draw half for kind 3 (the weather plate's: `0x0003a989` and
/// `0x0003b54d..0x0003b87d`), from the placement's `range`.
/// `skip_setup` is argument 7, `wants_result` argument 8 non-null.
/// Kinds 0 to 2 (`0x0003aaf3`, `0x0003ae5e`, `0x0003b296`), other
/// callers', are not ported.
pub fn kind3(range: &Range, colours: &Colours, skip_setup: bool, wants_result: bool, g: Globals) -> Plate {
    let mut ops = Vec::new();
    if !skip_setup {
        reset(&mut ops, g.target);
    }
    // 0x0003a989: the fixed-point near end split into the definition's
    // bytes.
    let v = {
        let f = (f64::from(range.near) * 65536.0).floor();
        if (-2147483648.0..2147483648.0).contains(&f) { f as i32 } else { i32::MIN }
    };
    let mut low = (v & 0xff) as u32;
    let mut high = v >> 8;
    if high > 0xff {
        high = 0xff;
        low = 0xff;
    }
    let rescaled = u32::from(range.rescaled);
    let lead = rescaled + 4;
    let tail = if range.rescaled { 4u32 } else { 24 };
    let mut w = Def { w: [0; 60], set: [false; 60] };
    for (a, v) in [
        (0x484f94, low << 24),
        (0x484ff4, lead << 24 | 0x20_1140),
        (0x484f6c, lead << 24 | 0x20_9140),
        (0x484f98, (high as u32) << 24),
        (0x484f70, tail << 24 | 0x20_1140),
        (0x484ff8, 0x2020),
        (0x485040, 1),
        (0x48503c, 0x11108),
        (0x484f90, 0xff_0000),
        (0x484fb0, 0xff00),
        (0x484ff0, 0x0801_0802),
        (0x48501c, 0x3045),
        (0x485020, 0xc00),
        (0x484fd4, 0xc00),
        (0x485024, 0x4d00),
        (0x484fd8, 0xd00),
        (0x484f9c, 0x0100_0000),
        (0x484ffc, 0xcc20_2d20),
        (0x485028, 0xc00),
        (0x484f74, 0xdd20_2df1),
        (0x484fdc, 0xd00),
    ] {
        word(&mut w, a, v);
    }
    // The mip level: the smallest n >= 1 with 2^n >= far, 0 for far <= 1.
    let far = range.far;
    let mut n = 0u32;
    if far > 1.0 {
        loop {
            n += 1;
            if !(far > (1u32 << n) as f32) {
                break;
            }
        }
    }
    if !skip_setup {
        ops.extend([
            Op::State(0x3b, 1),
            Op::State(0x3e, 0x8001),
            Op::State(0x3f, 0x303),
            Op::State(0x4a, 0x8006),
            Op::State(0x4b, 0xff_ffff),
            Op::State(0x8f, 2),
            Op::State(0x40, 0),
            Op::State(0x39, 0x203),
            Op::PushState0,
        ]);
    }
    let on = |b: bool| u32::from(b);
    let lvl = |hi: u32, lo: u32| -> u32 {
        // `(n > hi) ? 0x20 : (n <= lo ? 0 : 0x10)`, shifted into place.
        let x = if n > hi { 0x20 } else if n <= lo { 0 } else { 0x10 };
        x << 12 | 0xd00
    };
    let c = colours;
    word(&mut w, 0x485040, 0x8421);
    word(&mut w, 0x484fa0, pack_colour(c.rgb[0]));
    word(&mut w, 0x484fc0, pack_colour(c.rgb[1]));
    word(&mut w, 0x48502c, 0x3045);
    let a = u32::from(c.flag);
    let r = (a * 0x10 + 9) << 16 | (a * 0x10 + 0xa);
    word(&mut w, 0x485000, r << 8 | 0x1_0002);
    word(&mut w, 0x484f78, (on(n > 3) * 0x20) | 0x1d20_1d00);
    word(&mut w, 0x484fe0, lvl(2, 1));
    word(&mut w, 0x484fa4, pack_colour(c.rgb[2]));
    word(&mut w, 0x485030, 0x1045);
    word(&mut w, 0x485004, (a * 0x10 + 0xb) << 8 | 0x2425_0001);
    word(&mut w, 0x484f7c, (on(n > 6) * 0x20) | 0x1d20_1d00);
    word(&mut w, 0x484fe4, lvl(5, 4));
    let g1 = (f64::from(far) * 256.0 / f64::from(1u32 << n)) as f32;
    let hi = clamp_byte(g1);
    let g2 = (f64::from(far) * 0.5) as f32;
    let lo = clamp_byte(g2);
    word(&mut w, 0x484fc8, hi << 24 | lo);
    word(&mut w, 0x485008, 0x0425_0000);
    word(&mut w, 0x485034, 0xd0);
    word(&mut w, 0x484f80, 0x1d12_0c02);
    word(&mut w, 0x484f84, 0x1d2d_0000);
    word(&mut w, 0x484fec, 0xd0);
    word(&mut w, 0x484fe8, (if n == 0 { 0 } else { 0x10u32 }) << 12 | 0xd00);
    word(&mut w, 0x485014, pack_colour(c.rgb[3]));
    word(&mut w, 0x484f88, 0x1d01_0000);
    word(&mut w, 0x484f8c, 0x1d00);
    ops.push(Op::Define);
    // 0x0003b821..0x0003b87d.
    let mut result = None;
    if wants_result {
        let mut x = g.scale * range.near;
        if range.rescaled {
            x *= 1.0 / 256.0;
        }
        if g.limit < x || x.is_nan() || g.limit.is_nan() {
            result = Some(x);
        } else {
            if !skip_setup {
                ops.push(Op::State(0x8f, 0));
            }
            result = Some(g.limit);
        }
    }
    Plate { ops, define: w.w, written: w.set, result }
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

    #[test]
    fn a_colour_packs_to_rgb_bytes() {
        assert_eq!(pack_colour([1.0, 0.0, 0.5]), 0x00ff_0080);
    }

    #[test]
    fn the_weather_plate_definition_follows_its_range_and_far_end() {
        let range = Range { near: 0.5, far: 3.0, cap: 128.0, fade: 0.0, rescaled: false };
        let colours = Colours { flag: false, rgb: [[1.0, 1.0, 1.0]; 4] };
        let g = Globals { scale: 100.0, limit: 10.0, target: 2 };
        let p = kind3(&range, &colours, false, true, g);
        let at = |a: u32| p.define[((a - DEFINE) / 4) as usize];
        assert_eq!(at(0x484f98), 0x8000_0000);
        assert_eq!(at(0x484f70), 0x1820_1140);
        assert_eq!(at(0x484f78), 0x1d20_1d00);
        assert_eq!(at(0x484fe0), 0x10d00);
        assert_eq!(at(0x484fc8), 0xc000_0001);
        assert_eq!(at(0x485040), 0x8421);
        assert_eq!(at(0x484fa0), 0x00ff_ffff);
        assert_eq!(at(0x485000), (9 << 16 | 0xa) << 8 | 0x1_0002);
        // 100 * 0.5 is over the limit: it is the result, no state set.
        assert_eq!(p.result, Some(50.0));
        assert_eq!(p.ops.last(), Some(&Op::Define));
        // 42 of the 60 words are the function's; the rest are the buffer's.
        assert_eq!(p.written.iter().filter(|&&b| b).count(), 42);
        assert!(!p.written[0] && p.written[((0x484f98 - DEFINE) / 4) as usize]);
        assert_eq!(p.ops[0], Op::Bind(2));
        let q = kind3(&Range { near: 0.01, ..range }, &colours, true, true, g);
        assert_eq!(q.result, Some(10.0));
        assert!(!q.ops.contains(&Op::DeviceReset) && q.ops == [Op::Define]);
    }
}
