//! The light record update `c0c80` and its modulation step `c0b00` over
//! the [`LightTable`] (docs/engine/object-update.md, "Slice 5 bodies").
//! The arithmetic is SSE single precision in retail's operand order; the
//! three channel scalings of `c0b00` are x87 (`fild`, two `fmul`, `fstp`
//! to binary32), which with a 64 bit significand is one rounding of the
//! exact product ([`channel`]). The float environment is the default
//! one (round to nearest, no flush); the engine's hook checks that before
//! it calls in.
//!
//! What stays behind [`RecordHost`] is what retail calls out to and that
//! reads game state this crate does not hold: the object function
//! evaluators (`OBJECT_FUNCTION_VALUE`, `bad50`, `131d60`, `17ca10`,
//! `131c20`, `13b390`, `13bc00`, `13bb90`), the damage root `baf80` and the
//! two owner tests `b9ce0` and `b9d20`. Everything else (the path choice,
//! the radius and intensity blends, the attenuation by the owner's damage,
//! the clamps, the active-flag reconcile, the link and unlink calls) is
//! here.
//!
//! | Function | Rust |
//! |---|---|
//! | `c0c80` | [`update`] |
//! | `c0b00` | [`modulate`] |
//!
//! Element fields (`+` bytes): `02` flags (`u16`; bit 0 enabled, 1 active,
//! 3 linked, 4 timed), `04` definition, `14` created, `4c` owner, `50`
//! function argument, `54` function name (`ffff` none), `56` source,
//! `58` parent, `c4` intensity, `c8` attenuation, `cc` base, `d0` blend
//! weight, `d4` and `e0` the two colour triples, `ec` radius, `f0` and
//! `f4` the modulation outputs.

use sim_world::Handle;

use super::{LightHost, LightRecord, LightTable, field, flag, unlink};
use crate::table::ObjectTable;

const NONE: u32 = u32::MAX;

/// Element fields past [`field`].
pub mod at {
    pub const FUNCTION_ARG: usize = 0x50;
    pub const FUNCTION: usize = 0x54;
    pub const SOURCE: usize = 0x56;
    pub const INTENSITY: usize = 0xc4;
    pub const ATTENUATION: usize = 0xc8;
    pub const BASE: usize = 0xcc;
    pub const WEIGHT: usize = 0xd0;
    pub const COLOUR_A: usize = 0xd4;
    pub const COLOUR_B: usize = 0xe0;
    pub const RADIUS: usize = 0xec;
    pub const OUT: usize = 0xf0;
}

/// Bytes of a light definition the update reads (`+b6` is the last word).
pub const DEFINITION_LEN: usize = 0xe0;
/// The modulation block inside the definition.
const BLOCK: usize = 0xc0;
/// An object body's type byte and damage amount (`baf80`'s root).
const BODY_KIND: usize = 0xaa;
const BODY_DAMAGE: usize = 0x2b0;

/// A definition's offsets.
mod def {
    pub const FLAGS: usize = 0x00;
    pub const INTENSITY: usize = 0x08;
    pub const COLOUR_FUNCTION: usize = 0x34;
    /// The fade's two colour pairs, and the static colours.
    pub const FADE_B: (usize, usize) = (0x40, 0x4c);
    pub const FADE_A: (usize, usize) = (0x58, 0x64);
    pub const STATIC_B: usize = 0x4c;
    pub const STATIC_A: usize = 0x64;
    /// The radius endpoints.
    pub const RADIUS: usize = 0x70;
    pub const LIFETIME: usize = 0xb0;
    pub const FADE_FUNCTION: usize = 0xb6;
}

/// The game clock as the record update reads it: game time `+8` (tick),
/// `+2` (ticks per second, signed word) and `+4` (the tick length, binary32).
#[derive(Clone, Copy, Debug)]
pub struct GameTime {
    pub tick: i32,
    pub ticks_per_second: i16,
    pub scale: u32,
}

/// The image's constants the update reads: binary32 one (`45dbc0`), the
/// radius scale and cap (`45dc0c`, `45dc10`) and the byte scale (`45dbb4`).
#[derive(Clone, Copy, Debug)]
pub struct Constants {
    pub one: u32,
    pub radius_scale: u32,
    pub radius_max: u32,
    pub byte_scale: u32,
}

/// What an object function evaluator hands back: the value (`xmm0`) and,
/// for the brightness function, its data's flag byte `+1` and its range
/// words `+4` and `+8`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Evaluated {
    pub value: u32,
    pub flags: u8,
    pub low: u32,
    pub high: u32,
}

/// The animated path's result: the weight `OBJECT_FUNCTION_VALUE` writes
/// at `+d0` and the colours `131d60` writes at `+d4` and `+e0`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Animated {
    pub weight: u32,
    pub colour_a: [u32; 3],
    pub colour_b: [u32; 3],
}

/// The callees the update leaves to the guest and the game's data.
pub trait RecordHost: LightHost {
    /// The definition's first [`DEFINITION_LEN`] bytes (tag datum at `+04`).
    fn definition(&self, datum: u32) -> Vec<u8>;
    fn time(&self) -> GameTime;
    fn constants(&self) -> Constants;
    /// The object globals' first byte `[[4de2f4]]` is set (`c0c80` links a
    /// light that turns on only then).
    fn link_enabled(&self) -> bool;
    /// `OBJECT_FUNCTION_VALUE`, `bad50` and the two `131d60` calls: the
    /// animated path's weight and colours for `owner`.
    fn animate(&mut self, light: &LightRecord, definition: &[u8], owner: u32) -> Animated;
    /// `17ca10`: the fade curve of function `function` at `t`, over `denominator`.
    fn fade_curve(&mut self, function: u16, t: u32, denominator: u32) -> u32;
    /// `131c20`: colour function `function` between `a` and `b` at `weight`.
    fn interpolate(&mut self, function: u32, a: [u32; 3], b: [u32; 3], weight: u32) -> [u32; 3];
    /// `baf80`: the root of `owner`'s object tree.
    fn root(&mut self, owner: u32) -> u32;
    /// `b9ce0`.
    fn owner_test(&mut self, owner: u32) -> bool;
    /// `b9d20`.
    fn owner_test_2(&mut self, owner: u32) -> bool;
    /// `13b390` for the function at block offset `at` (`08` or `10`).
    fn evaluate(&mut self, block: &[u8], at: usize, phase: u32, input: u32) -> Evaluated;
    /// `13bc00`: the packed colour of the function at block offset `10`.
    fn colour(&mut self, block: &[u8], value: u32) -> u32;
    /// `13bb90` for the source at block offset `at` (`18` or `20`).
    fn source(&mut self, block: &[u8], at: usize, phase: u32, input: u32) -> u32;
}

fn f(bits: u32) -> f32 {
    f32::from_bits(bits)
}
fn b(v: f32) -> u32 {
    v.to_bits()
}
fn dw(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}
fn flags16(r: &LightRecord) -> u16 {
    r.u16(field::FLAGS)
}
fn set_flags16(r: &mut LightRecord, v: u16) {
    r.bytes[field::FLAGS..field::FLAGS + 2].copy_from_slice(&v.to_le_bytes());
}
fn triple(r: &mut LightRecord, at: usize, v: [u32; 3]) {
    for (i, w) in v.into_iter().enumerate() {
        r.set_u32(at + 4 * i, w);
    }
}
fn read_triple(bytes: &[u8], at: usize) -> [u32; 3] {
    [dw(bytes, at), dw(bytes, at + 4), dw(bytes, at + 8)]
}

/// The radius blend of the animated and fade paths.
fn set_radius(r: &mut LightRecord, definition: &[u8], t: u32, one: u32) {
    r.set_u32(at::RADIUS, blend(dw(definition, def::RADIUS), dw(definition, def::RADIUS + 4), t, one));
}

/// `(1 - t) * start + end * t`, in retail's order.
fn blend(start: u32, end: u32, t: u32, one: u32) -> u32 {
    b((f(one) - f(t)) * f(start) + f(end) * f(t))
}

/// `clamp(v, 0, 1)` as the clamp ladder writes it: below zero is zero,
/// above one is one, unordered stays.
fn unit(v: f32, one: f32) -> f32 {
    if 0.0 > v {
        0.0
    } else if v > one {
        one
    } else {
        v
    }
}

/// `fild n; fmul a; fmul scale; fstp dword`: the exact product rounded
/// once to binary32 (the x87's 64 bit significand holds every partial
/// product of an 8 bit integer and two binary32 words).
pub fn channel(n: u32, a: u32, scale: u32) -> u32 {
    let (fa, fs) = (f(a), f(scale));
    if !fa.is_finite() || !fs.is_finite() || fa.is_subnormal() || fs.is_subnormal() {
        unimplemented!("lights: modulate: channel operand {a:#x} * {scale:#x} is not a finite binary32");
    }
    let negative = (a ^ scale) >> 31 != 0;
    if n == 0 || fa == 0.0 || fs == 0.0 {
        return if negative { 0x8000_0000 } else { 0 };
    }
    let mant = |x: u32| u128::from((x & 0x007f_ffff) | 0x0080_0000);
    let exp = |x: u32| ((x >> 23) & 0xff) as i32 - 127 - 23;
    let product = u128::from(n) * mant(a) * mant(scale);
    let mut e = exp(a) + exp(scale);
    let bits = 128 - product.leading_zeros();
    let mut m = product;
    if bits > 24 {
        let shift = bits - 24;
        let rest = m & ((1u128 << shift) - 1);
        m >>= shift;
        e += shift as i32;
        let half = 1u128 << (shift - 1);
        if rest > half || (rest == half && m & 1 == 1) {
            m += 1;
        }
    }
    let value = (m as f64) * 2f64.powi(e);
    let v = value as f32;
    if !v.is_normal() {
        unimplemented!("lights: modulate: channel result {value} leaves binary32's normal range");
    }
    b(if negative { -v } else { v })
}

/// `c0b00`: scale the radius by the brightness function, the colour
/// triple by the colour function and compute the two source outputs.
/// `block` is the definition's [`DEFINITION_LEN`] bytes from `+c0` on
/// (`block[0..0x20]`), `handle` the light's handle.
pub fn modulate<H: RecordHost>(
    host: &mut H,
    light: &mut LightRecord,
    block: &[u8],
    handle: u32,
    time: GameTime,
    one: u32,
    byte_scale: u32,
) {
    let tick = time.tick;
    let phase = if block[0] & 1 != 0 {
        b(tick as f32 * f(time.scale))
    } else {
        let n = (handle.wrapping_mul(0x1387) & 0x7fff_ffff) as i32;
        let d = i32::from(time.ticks_per_second).wrapping_mul(0x3c);
        if d == 0 {
            unimplemented!("lights: modulate: a game time with no ticks per second divides by zero (guest fault at c0b3b)");
        }
        let at = n.wrapping_rem(d).wrapping_add(tick);
        b(at as f32 * f(time.scale))
    };
    let input = light.u32(at::WEIGHT);
    if dw(block, 4) as i32 > 0 {
        let e = host.evaluate(block, 0x08, phase, input);
        let mut x = f(e.value);
        if e.flags & 0xf0 == 0 {
            let below = 0.0 > x;
            let above = x > f(one);
            if below {
                x = 0.0;
            } else if above {
                x = f(one);
            }
            let (low, high) = (f(e.low), f(e.high));
            x = (high - low) * x + low;
        }
        light.set_u32(at::RADIUS, b(f(light.u32(at::RADIUS)) * x));
    }
    if dw(block, 0xc) as i32 > 0 {
        let e = host.evaluate(block, 0x10, phase, input);
        let packed = host.colour(block, e.value);
        for (i, shift) in [16u32, 8, 0].into_iter().enumerate() {
            let n = (packed >> shift) & 0xff;
            let at = at::COLOUR_A + 4 * i;
            light.set_u32(at, channel(n, light.u32(at), byte_scale));
        }
    }
    if dw(block, 0x14) as i32 > 0 {
        let first = host.source(block, 0x18, phase, input);
        light.set_u32(at::OUT, first);
        let second = host.source(block, 0x20, phase, input);
        light.set_u32(at::OUT + 4, second);
    }
}

/// `c0c80 (light)`: evaluate the light's colour, radius and intensity,
/// attenuate by the owner's damage, clamp, and reconcile the active bit
/// (and with it the link) with the result. A light that is not live does
/// nothing.
pub fn update<H: RecordHost>(
    lights: &mut LightTable,
    objects: &ObjectTable,
    host: &mut H,
    light: Handle,
) {
    let Some(r) = lights.get(light) else { return };
    let mut r = r.clone();
    let definition = host.definition(r.u32(field::DEFINITION));
    let (time, k) = (host.time(), host.constants());
    let owner = match (r.u32(field::OWNER), r.u32(field::PARENT)) {
        (NONE, NONE) => NONE,
        (NONE, parent) => parent,
        (owner, _) => owner,
    };
    // The owner's body pointer: nonzero for any slot an object holds.
    let body_live = owner != NONE && objects.handle_at((owner & 0xffff) as u16).is_some();

    if r.u16(at::FUNCTION) != 0xffff {
        let a = host.animate(&r, &definition, owner);
        r.set_u32(at::WEIGHT, a.weight);
        triple(&mut r, at::COLOUR_A, a.colour_a);
        triple(&mut r, at::COLOUR_B, a.colour_b);
        set_radius(&mut r, &definition, a.weight, k.one);
    } else if flags16(&r) & u16::from(flag::TIMED) != 0 {
        let ticks = time.tick.wrapping_sub(r.u32(field::CREATED) as i32) as f32;
        let denominator = f32::from(time.ticks_per_second) * f(dw(&definition, def::LIFETIME));
        let function = u16::from_le_bytes([definition[def::FADE_FUNCTION], definition[def::FADE_FUNCTION + 1]]);
        let curve = host.fade_curve(function, b(ticks / denominator), b(denominator));
        let weight = b((f(k.one) - f(curve)) * f(r.u32(at::BASE)));
        r.set_u32(at::WEIGHT, weight);
        let function = dw(&definition, def::COLOUR_FUNCTION);
        let colour = |h: &mut H, (from, to): (usize, usize)| {
            h.interpolate(function, read_triple(&definition, from), read_triple(&definition, to), weight)
        };
        let a = colour(host, def::FADE_A);
        triple(&mut r, at::COLOUR_A, a);
        let c = colour(host, def::FADE_B);
        triple(&mut r, at::COLOUR_B, c);
        set_radius(&mut r, &definition, weight, k.one);
    } else {
        r.set_u32(at::WEIGHT, r.u32(at::BASE));
        triple(&mut r, at::COLOUR_A, read_triple(&definition, def::STATIC_A));
        triple(&mut r, at::COLOUR_B, read_triple(&definition, def::STATIC_B));
        r.set_u32(at::RADIUS, dw(&definition, def::RADIUS + 4));
    }

    modulate(host, &mut r, &definition[BLOCK..], light.raw(), time, k.one, k.byte_scale);

    // The owner's damage dims the light (`baf80`, then the root's body).
    r.set_u32(at::ATTENUATION, 0);
    if body_live {
        let root = host.root(owner);
        if let Some(h) = objects.handle_at((root & 0xffff) as u16) {
            let body = &objects.get(h).expect("live").body;
            if body.len() < BODY_DAMAGE + 4 {
                unimplemented!("lights: update: root object {root:#x} has no damage word in its body");
            }
            if (1u32 << (body[BODY_KIND] & 31)) & 3 != 0 {
                let amount = f(dw(body, BODY_DAMAGE));
                if amount > 0.0 && (definition[def::FLAGS] as i8) >= 0 {
                    let atten = unit(amount, f(k.one));
                    r.set_u32(at::ATTENUATION, b(atten));
                    let inv = f(k.one) - atten;
                    for i in 0..6 {
                        let p = at::COLOUR_A + 4 * i;
                        let v = f(r.u32(p));
                        r.set_u32(p, b(if i < 4 { v * inv } else { inv * v }));
                    }
                }
            }
        }
    }

    // Clamp.
    let one = f(k.one);
    for i in 0..6 {
        let p = at::COLOUR_A + 4 * i;
        r.set_u32(p, b(unit(f(r.u32(p)), one)));
    }
    let scaled = f(r.u32(at::RADIUS)) * f(k.radius_scale);
    let radius = if 0.0 > scaled {
        0.0
    } else if scaled > f(k.radius_max) {
        f(k.radius_max)
    } else {
        scaled
    };
    r.set_u32(at::RADIUS, b(radius));
    let t = r.u32(at::WEIGHT);
    let intensity = blend(dw(&definition, def::INTENSITY), dw(&definition, def::INTENSITY + 4), t, k.one);
    r.set_u32(at::INTENSITY, intensity);
    let flags = flags16(&r);
    let lit = flags & 1 != 0
        && f(intensity) > 0.0
        && (0..6).any(|i| f(r.u32(at::COLOUR_A + 4 * i)) != 0.0);

    let (active, linked) = (flags & u16::from(flag::ACTIVE) != 0, flags & u16::from(flag::LINKED) != 0);
    if lit {
        if active {
            *lights.get_mut(light).expect("live") = r;
            return;
        }
        set_flags16(&mut r, flags | u16::from(flag::ACTIVE));
        *lights.get_mut(light).expect("live") = r;
        let link = owner == NONE || (!host.owner_test(owner) && host.owner_test_2(owner));
        if link && host.link_enabled() {
            host.link(lights, light);
        }
    } else if active {
        if linked {
            *lights.get_mut(light).expect("live") = r;
            unlink(lights, host, light);
            let r = lights.get_mut(light).expect("a host callee frees no light");
            let flags = flags16(r);
            set_flags16(r, flags & !u16::from(flag::ACTIVE));
        } else {
            set_flags16(&mut r, flags & !u16::from(flag::ACTIVE));
            *lights.get_mut(light).expect("live") = r;
        }
    } else {
        *lights.get_mut(light).expect("live") = r;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::ObjectRecord;

    const DEF: u32 = 0xd0d0;

    fn bits(v: f32) -> u32 {
        v.to_bits()
    }

    #[derive(Default)]
    struct Host {
        calls: Vec<String>,
        definition: Vec<u8>,
        enabled: bool,
        tests: (bool, bool),
    }

    impl LightHost for Host {
        fn record_update(&mut self, _: &mut LightTable, _: Handle) {}
        fn link(&mut self, lights: &mut LightTable, l: Handle) {
            self.calls.push("link".into());
            let r = lights.get_mut(l).unwrap();
            r.set_flags(r.flags() | flag::LINKED);
        }
        fn list_remove(&mut self, _: Handle, record: u32) {
            self.calls.push(format!("remove {record:#x}"));
        }
        fn lifetime(&self, _: u32) -> u32 {
            0
        }
    }

    impl RecordHost for Host {
        fn definition(&self, datum: u32) -> Vec<u8> {
            assert_eq!(datum, DEF);
            self.definition.clone()
        }
        fn time(&self) -> GameTime {
            GameTime { tick: 100, ticks_per_second: 30, scale: bits(1.0 / 30.0) }
        }
        fn constants(&self) -> Constants {
            Constants { one: bits(1.0), radius_scale: bits(1.0), radius_max: bits(8.0), byte_scale: bits(1.0) }
        }
        fn link_enabled(&self) -> bool {
            self.enabled
        }
        fn animate(&mut self, _: &LightRecord, _: &[u8], _: u32) -> Animated {
            unimplemented!()
        }
        fn fade_curve(&mut self, function: u16, t: u32, denominator: u32) -> u32 {
            self.calls.push(format!("fade {function} {} {}", f(t), f(denominator)));
            bits(0.25)
        }
        fn interpolate(&mut self, _: u32, a: [u32; 3], _: [u32; 3], _: u32) -> [u32; 3] {
            a
        }
        fn root(&mut self, owner: u32) -> u32 {
            owner
        }
        fn owner_test(&mut self, _: u32) -> bool {
            self.tests.0
        }
        fn owner_test_2(&mut self, _: u32) -> bool {
            self.tests.1
        }
        fn evaluate(&mut self, _: &[u8], _: usize, _: u32, _: u32) -> Evaluated {
            unimplemented!()
        }
        fn colour(&mut self, _: &[u8], _: u32) -> u32 {
            unimplemented!()
        }
        fn source(&mut self, _: &[u8], _: usize, _: u32, _: u32) -> u32 {
            unimplemented!()
        }
    }

    /// A static definition: intensity 1 to 3, colour A `(0.5, 0.25, 1)`,
    /// radius endpoint 2.
    fn host() -> Host {
        let mut d = vec![0u8; DEFINITION_LEN];
        let put = |d: &mut Vec<u8>, at: usize, v: f32| d[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut d, 0x08, 1.0);
        put(&mut d, 0x0c, 3.0);
        for (i, v) in [0.5f32, 0.25, 1.0].into_iter().enumerate() {
            put(&mut d, 0x64 + 4 * i, v);
        }
        put(&mut d, 0x74, 2.0);
        Host { definition: d, enabled: true, ..Host::default() }
    }

    fn light(flags: u16, owner: u32) -> LightRecord {
        let mut r = LightRecord::default();
        r.bytes[2..4].copy_from_slice(&flags.to_le_bytes());
        r.set_u32(field::DEFINITION, DEF);
        r.set_u32(field::OWNER, owner);
        r.set_u32(field::PARENT, NONE);
        r.set_u32(at::FUNCTION & !3, 0);
        r.bytes[at::FUNCTION..at::FUNCTION + 2].copy_from_slice(&0xffffu16.to_le_bytes());
        r.set_u32(at::BASE, bits(0.5));
        r.set_u32(field::LIST, 0x77);
        r
    }

    #[test]
    fn a_static_enabled_light_blends_clamps_turns_on_and_links() {
        let mut t = LightTable::new(4, 0x696c);
        let h = t.alloc(light(1, NONE)).unwrap();
        let mut host = host();
        update(&mut t, &ObjectTable::default(), &mut host, h);
        let r = t.get(h).unwrap();
        assert_eq!(f(r.u32(at::WEIGHT)), 0.5);
        // (1 - 0.5) * 1 + 3 * 0.5
        assert_eq!(f(r.u32(at::INTENSITY)), 2.0);
        assert_eq!(f(r.u32(at::RADIUS)), 2.0);
        assert_eq!([f(r.u32(at::COLOUR_A)), f(r.u32(at::COLOUR_A + 4)), f(r.u32(at::COLOUR_A + 8))], [0.5, 0.25, 1.0]);
        assert_eq!(f(r.u32(at::ATTENUATION)), 0.0);
        assert_eq!(flags16(r), 1 | 2 | 8);
        assert_eq!(host.calls, ["link"]);
    }

    #[test]
    fn the_object_phase_byte_gates_the_link_and_a_disabled_light_stays_off() {
        let mut t = LightTable::new(4, 0x696c);
        let (on, off) = (t.alloc(light(1, NONE)).unwrap(), t.alloc(light(0, NONE)).unwrap());
        let mut host = host();
        host.enabled = false;
        update(&mut t, &ObjectTable::default(), &mut host, on);
        update(&mut t, &ObjectTable::default(), &mut host, off);
        assert_eq!(flags16(t.get(on).unwrap()), 1 | 2);
        assert_eq!(flags16(t.get(off).unwrap()), 0);
        assert!(host.calls.is_empty());
    }

    #[test]
    fn a_light_that_goes_dark_leaves_the_lists_and_loses_its_active_bit() {
        let mut t = LightTable::new(4, 0x696c);
        let h = t.alloc(light(1 | 2 | 8, NONE)).unwrap();
        let mut host = host();
        // Colour A is black: the light has nothing to show.
        for i in 0..3 {
            host.definition[0x64 + 4 * i..0x68 + 4 * i].copy_from_slice(&0f32.to_le_bytes());
        }
        update(&mut t, &ObjectTable::default(), &mut host, h);
        assert_eq!(flags16(t.get(h).unwrap()), 1);
        assert_eq!(host.calls, ["remove 0x77"]);
    }

    #[test]
    fn an_owned_light_links_only_when_the_first_owner_test_fails_and_the_second_holds() {
        let mut objects = ObjectTable::new(4, 0x626f);
        let owner = objects
            .alloc(ObjectRecord {
                body_size: 0x2b4,
                body_addr: 0x100,
                body: vec![0; BODY_DAMAGE + 4],
                ..ObjectRecord::default()
            })
            .unwrap();
        for (tests, linked) in [((false, true), true), ((true, true), false), ((false, false), false)] {
            let mut t = LightTable::new(4, 0x696c);
            let h = t.alloc(light(1, owner.raw())).unwrap();
            let mut host = host();
            host.tests = tests;
            update(&mut t, &objects, &mut host, h);
            assert_eq!(host.calls.len(), usize::from(linked), "{tests:?}");
        }
    }

    #[test]
    fn a_damaged_owner_dims_the_colours_and_records_the_attenuation() {
        let mut body = vec![0u8; BODY_DAMAGE + 4];
        body[BODY_KIND] = 0;
        body[BODY_DAMAGE..].copy_from_slice(&0.25f32.to_le_bytes());
        let mut objects = ObjectTable::new(4, 0x626f);
        let owner = objects
            .alloc(ObjectRecord { body_size: body.len() as u16, body_addr: 0x100, body, ..ObjectRecord::default() })
            .unwrap();
        let mut t = LightTable::new(4, 0x696c);
        let h = t.alloc(light(1, owner.raw())).unwrap();
        let mut host = host();
        update(&mut t, &objects, &mut host, h);
        let r = t.get(h).unwrap();
        assert_eq!(f(r.u32(at::ATTENUATION)), 0.25);
        // 0.75 of 0.5, 0.25 and 1.
        assert_eq!([f(r.u32(at::COLOUR_A)), f(r.u32(at::COLOUR_A + 4)), f(r.u32(at::COLOUR_A + 8))], [0.375, 0.1875, 0.75]);
    }

    #[test]
    fn a_timed_light_fades_by_the_curve_and_the_base_weight() {
        let mut t = LightTable::new(4, 0x696c);
        let mut r = light(1 | 0x10, NONE);
        r.set_u32(field::CREATED, 40);
        let h = t.alloc(r).unwrap();
        let mut host = host();
        // Lifetime 2 s at 30 ticks per second: 60 ticks; 60 elapsed.
        host.definition[def::LIFETIME..def::LIFETIME + 4].copy_from_slice(&2.0f32.to_le_bytes());
        host.definition[def::FADE_FUNCTION] = 7;
        update(&mut t, &ObjectTable::default(), &mut host, h);
        assert_eq!(host.calls[0], "fade 7 1 60");
        // (1 - 0.25) * base 0.5
        assert_eq!(f(t.get(h).unwrap().u32(at::WEIGHT)), 0.375);
    }

    #[test]
    fn a_channel_is_one_rounding_of_the_exact_product() {
        assert_eq!(f(channel(200, bits(0.75), bits(1.0))), 150.0);
        assert_eq!(f(channel(255, bits(2.0), bits(0.5))), 255.0);
        assert_eq!(channel(0, bits(3.0), bits(1.0)), 0);
        assert_eq!(channel(0, bits(-3.0), bits(1.0)), 0x8000_0000);
        // 3 * (1 + 2^-23)^2 = 3 + 3 * 2^-22 + 3 * 2^-46: the tail rounds away.
        let a = f32::from_bits(1.0f32.to_bits() + 1);
        assert_eq!(channel(3, bits(a), bits(a)), (3.0f32 + 3.0 * 2f32.powi(-22)).to_bits());
    }
}
