//! The motion sensor's draw calls, instruction for instruction
//! (docs/graphics/native-hud-overlay.md, "The motion sensor in Rust"):
//! where the HUD render `0x0013954b` puts the sensor (`0x00139614`..
//! `0x00139685`), and what `0x001a2823` then asks to be drawn: one blip
//! draw `0x00254490` (through `0x001a2197`) for each blip of the ring's
//! ten frames, from the ring's start, then the composite `0x002548f0`.
//!
//! (Code, `h2 disasm 0x001a2823 100`, `h2 disasm 0x001a2197 40`.) The
//! sensor keeps ten frames of blips for each local user at
//! `[0x0051e994] + user * 0x2f0`, `0x44` bytes a frame: up to sixteen
//! blips of four bytes (x, y as `i8`, its type, its size index as `i8`;
//! type 0 is no blip), then the count at `+0x40`. The frame the ring
//! starts at is `[[0x0051e994] + 0xbc0]`. Step `i` of ten reads frame
//! `(start + i) % 10` with `n = 10 - i`:
//!
//! | Value | Guest | Here |
//! |---|---|---|
//! | `a` | `n * 0.1` (`0x0045dc68`), SSE | [`Sse`] |
//! | the blip's shade | `a * a`, SSE | the blip draw's second argument |
//! | the frame's swell | `pow(1 - a, 3.5)` (`0x0045e1c8`) `* 7 + 1` (`0x0045dda8`, `0x0045dbc0`), x87 | [`X87`], [`function_eval::pow`] |
//! | the blip's size | `base * swell + [0x0047ff9c + 4 * size index]`, SSE; `base` is 1, or for type 6 `(sin(ticks * seconds a tick * pi) + 1) / 30 + 1` (`[0x00510c54]`, x87) | the same |
//! | its colour | `0x00445320 + 12 * type`, an address | the same |
//!
//! The SSE arithmetic keeps the original's operand order and the x87
//! parts run on the x87 model, so each value has the guest's bits.

use crate::function_eval::{self, fld_m64};
use crate::light_constants::{Error, Sse, X87};
use guest_layout::memory::GuestMemory;

use x87::abi::arith_op;

/// The HUD's window: top, left, bottom, right (`i16`).
pub const WINDOW: u32 = 0x004b_9dd0;
/// The HUD's safe rectangle: top, left, bottom, right (`i16`).
pub const SAFE: u32 = 0x004b_9dd8;
/// The sensor's centre for each screen mode, `i16` x and y from the safe
/// rectangle's bottom-left corner (`game::hud_swap::SENSOR_CENTRES`).
pub const CENTRES: u32 = 0x0047_ff40;
/// Each local user's camera: `0x140` bytes, its update function at `+0xc`
/// and its blend at `+4`.
pub const CAMERAS: u32 = 0x004e_8c44;
pub const CAMERA_LEN: u32 = 0x140;
/// The sensor's globals: its blips, then the ring's start at `+0xbc0`.
pub const SENSOR: u32 = 0x0051_e994;
pub const USER_LEN: u32 = 0x2f0;
pub const RING_START: u32 = 0xbc0;
pub const FRAME_LEN: u32 = 0x44;
pub const FRAME_COUNT: u32 = 0x40;
/// Frames the ring holds, and so draws.
pub const FRAMES: i32 = 10;
/// The composite's second argument.
pub const COMPOSITE_VALUE: u32 = 0x0051_e98c;
/// The game time: seconds a tick (`f32`) at `+4`, ticks (`i32`) at `+8`.
pub const TIME: u32 = 0x0051_0c54;
/// A type's colour: three `f32`.
pub const COLOURS: u32 = 0x0044_5320;
/// A size index's size (`f32`).
pub const SIZES: u32 = 0x0047_ff9c;
/// The pulsing type.
pub const PULSE_TYPE: u8 = 6;

const STEP: u32 = 0x0045_dc68;
const EXPONENT: u32 = 0x0045_e1c8;
const SWELL: u32 = 0x0045_dda8;
const ONE: u32 = 0x0045_dbc0;
const PI: u32 = 0x0045_dddc;
const PULSE: u32 = 0x0045_dc6c;
const BLEND_FIRST_PERSON: u32 = 0x0045_dbd8;
/// 1.0 as `fld1` loads it.
const FLD1: u32 = 0x3f80_0000;

/// The camera update functions `0x00155760` names.
const CAMERA_FIRST_PERSON: u32 = 0x0023_c110;
const CAMERA_FOLLOWING: u32 = 0x0023_cbb0;
const CAMERA_DEAD: u32 = 0x0016_c840;
const CAMERA_FLYING: u32 = 0x0023_d090;
/// The byte that makes the flying camera a mode-2 camera.
const FLYING_MODE: u32 = 0x0051_ec10;

/// A blip count past this reads more than the ring: the port stops.
const MOST_BLIPS: i32 = 0x100;

/// One blip draw `0x00254490`: its arguments, floats as bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Blip {
    /// x, y: the blip's bytes as floats, what its first argument points at.
    pub position: [u32; 2],
    pub shade: u32,
    pub size: u32,
    /// The colour's address.
    pub colour: u32,
    /// Whether the type pulses (the fifth argument's low byte).
    pub pulse: bool,
}

/// What one call of `0x001a2823` draws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sensor {
    /// The point the call was given plus the window's corner, as floats:
    /// what the composite's first argument points at.
    pub centre: [u32; 2],
    /// The blip draws, in order.
    pub blips: Vec<Blip>,
    /// The composite's second argument.
    pub value: u32,
}

fn i16_at(mem: &dyn GuestMemory, at: u32) -> Result<i32, Error> {
    Ok(i32::from(mem.u16(at)? as i16))
}

fn cvtt(sse: &mut Sse, v: u32) -> Result<i32, Error> {
    Sse::lane(simd::cvttss2si(&mut sse.mxcsr, v).map(u128::from)).map(|v| v as i32)
}

/// `0x0013a690(4)`: the screen mode the window is, 0 (full), 1 (half)
/// or 2 (quarter), by its 16-bit width and height against 640 and 480.
pub fn screen_mode(mem: &dyn GuestMemory) -> Result<u32, Error> {
    let width = mem.u32(WINDOW + 6)?.wrapping_sub(mem.u32(WINDOW + 2)?) as u16 as i16;
    let height = mem.u32(WINDOW + 4)?.wrapping_sub(mem.u32(WINDOW)?) as u16 as i16;
    // With an argument of 4 the split-screen check (`0x00016a90`) does
    // not change the mode.
    Ok(match (width >= 640, height >= 480) {
        (true, true) => 0,
        (false, false) => 2,
        _ => 1,
    })
}

/// The point the HUD render gives `0x001a2823` (`0x00139614`..
/// `0x0013967a`): anchor 2 (the safe rectangle's left and bottom), plus
/// the screen mode's centre, less the window's corner, each truncated.
pub fn point(mem: &dyn GuestMemory, mxcsr: u32) -> Result<[i16; 2], Error> {
    let mut sse = Sse { mxcsr };
    let mode = screen_mode(mem)?;
    let anchor = [sse.int(i16_at(mem, SAFE + 2)?)?, sse.int(i16_at(mem, SAFE + 4)?)?];
    let entry = CENTRES + 4 * mode;
    let mut out = [0i16; 2];
    for (axis, (offset, corner)) in [(0, WINDOW + 2), (2, WINDOW)].into_iter().enumerate() {
        let v = sse.int(i16_at(mem, entry + offset)?)?;
        let v = sse.add(v, anchor[axis])?;
        let corner = sse.int(i16_at(mem, corner)?)?;
        let v = sse.sub(v, corner)?;
        out[axis] = cvtt(&mut sse, v)? as i16;
    }
    Ok(out)
}

/// `0x00155760`: the local user's camera mode. 0 first person (with no
/// blend), 1 following, 2 dead or flying with `[0x0051ec10]` set, else 3.
pub fn camera_mode(mem: &dyn GuestMemory, mxcsr: u32, user: u32) -> Result<u32, Error> {
    let camera = CAMERAS.wrapping_add(user.wrapping_mul(CAMERA_LEN));
    Ok(match mem.u32(camera.wrapping_add(0xc))? {
        CAMERA_FIRST_PERSON => {
            let mut sse = Sse { mxcsr };
            let blend = mem.u32(camera.wrapping_add(4))?;
            if sse.equal(blend, mem.u32(BLEND_FIRST_PERSON)?)? { 0 } else { 3 }
        }
        CAMERA_FOLLOWING => 1,
        CAMERA_DEAD => 2,
        CAMERA_FLYING if mem.u8(FLYING_MODE)? != 0 => 2,
        _ => 3,
    })
}

/// `0x001a2197`'s size base for a pulsing blip: `(sin(ticks * seconds *
/// pi) + 1) * (1 / 30) + 1`, stored as `f32`.
fn pulse(mem: &dyn GuestMemory, control: u16) -> Result<u32, Error> {
    let time = mem.u32(TIME)?;
    let mut x = X87::new(control);
    x.fild(mem.i32(time.wrapping_add(8))?)?;
    x.fmul_m(mem.u32(time.wrapping_add(4))?)?;
    x.fmul_m(mem.u32(PI)?)?;
    x.fsin()?;
    x.fld(mem.u32(ONE)?)?;
    x.fld_st(1)?;
    x.arith_st(arith_op::ADD, 1)?;
    x.fmul_m(mem.u32(PULSE)?)?;
    x.arith_st(arith_op::ADD, 1)?;
    x.fstp_m32()
}

/// `0x001a2823` for local user `user` given `point`, under the caller's
/// MXCSR and x87 control word: what it draws, or `None` when the camera
/// mode is 2 or 3 and it draws nothing.
pub fn draw(mem: &dyn GuestMemory, mxcsr: u32, control: u16, user: u32, point: [i16; 2]) -> Result<Option<Sensor>, Error> {
    if matches!(camera_mode(mem, mxcsr, user)?, 2 | 3) {
        return Ok(None);
    }
    let mut sse = Sse { mxcsr };
    let centre = [
        sse.int(i32::from(point[0]).wrapping_add(i16_at(mem, WINDOW + 2)?))?,
        sse.int(i32::from(point[1]).wrapping_add(i16_at(mem, WINDOW)?))?,
    ];
    let globals = mem.u32(SENSOR)?;
    let blips_at = globals.wrapping_add(user.wrapping_mul(USER_LEN));
    let one = mem.u32(ONE)?;
    let mut blips = Vec::new();
    for i in 0..FRAMES {
        let n = FRAMES - i;
        let start = mem.i32(globals.wrapping_add(RING_START))?;
        let slot = start.wrapping_add(i) % FRAMES;
        let a = sse.int(n)?;
        let a = sse.mul(a, mem.u32(STEP)?)?;
        let mut x = X87::new(control);
        x.fld(FLD1)?;
        x.arith_m(arith_op::SUB, a)?;
        let shade = sse.mul(a, a)?;
        let mut exponent = [0u8; 8];
        mem.read(EXPONENT, &mut exponent)?;
        fld_m64(&mut x, u64::from_le_bytes(exponent))?;
        function_eval::pow(mem, &mut x)?;
        x.fmul_m(mem.u32(SWELL)?)?;
        x.fadd_m(one)?;
        let swell = x.fstp_m32()?;
        let frame = blips_at.wrapping_add((slot as u32).wrapping_mul(FRAME_LEN));
        let count = mem.i32(frame.wrapping_add(FRAME_COUNT))?;
        if count > MOST_BLIPS {
            return Err(Error::Unbounded);
        }
        for j in 0..count.max(0) as u32 {
            let at = frame.wrapping_add(4 * j);
            let mut blip = [0u8; 4];
            mem.read(at, &mut blip)?;
            let [bx, by, kind, size] = blip;
            if kind == 0 {
                continue;
            }
            let position = [sse.int(i32::from(bx as i8))?, sse.int(i32::from(by as i8))?];
            let table = mem.u32(SIZES.wrapping_add((i32::from(size as i8) as u32).wrapping_mul(4)))?;
            let colour = COLOURS.wrapping_add((i32::from(kind as i8) as u32).wrapping_mul(12));
            let base = if kind == PULSE_TYPE { pulse(mem, control)? } else { one };
            let size = sse.mul(base, swell)?;
            let size = sse.add(size, table)?;
            blips.push(Blip { position, shade, size, colour, pulse: kind == PULSE_TYPE });
        }
    }
    // `fld dword; fstp dword` onto the stack.
    let mut x = X87::new(control);
    x.fld(mem.u32(COMPOSITE_VALUE)?)?;
    let value = x.fstp_m32()?;
    Ok(Some(Sensor { centre, blips, value }))
}

/// The HUD's globals: `+0x110` whether `+0x114` holds the HUD's
/// interface, whose `+0x64` and `+0x6c` are the blip bitmaps' tag indices.
pub const HUD_GLOBALS: u32 = 0x004e_034c;
/// The tag instances, `0x10` bytes each, the definition at `+8`.
pub const TAG_INSTANCES: u32 = 0x004e_3b44;
/// The texture cache's stamp a resident bitmap's `+0x70` is past.
pub const CACHE_STAMP: u32 = 0x004e_6488;
/// The byte that lets a blip draw (`0x00254560`).
pub const BLIPS_DRAW: u32 = 0x0051_ec9a;
/// The half size factor (`0x0025457c`) and the blip's depth (`0x002546fe`).
const HALF: u32 = 0x0045_dc0c;
const DEPTH: u32 = 0x0045_dd40;

/// One blip draw `0x00254490` as its body makes it, from its arguments
/// (Code, `h2 disasm 0x00254490 200`).
#[derive(Clone, Debug, PartialEq)]
pub struct BlipDraw {
    /// `[0x0051ec9a]` is set: the body reaches its state and its quad.
    pub draws: bool,
    /// The bitmap's texture, for stage 0 (`[0x0051f3d8]`).
    pub texture: u32,
    /// The bitmap's `+4` and `+6`, widened.
    pub width: i32,
    pub height: i32,
    /// The quad: the colour (register 5), the texture coordinate
    /// (register 1) and the position (register 0) of each vertex.
    pub vertices: crate::ui_backdrop::Fan<3>,
    /// MXCSR after the body's SSE operations.
    pub mxcsr: u32,
}

/// Why Rust cannot make a blip's draw as its body does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlipRefusal {
    /// A bitmap is missing or not resident: the body asks the texture
    /// cache (`0x0012ccf0`, `0x0012ce00`).
    Bitmap,
    /// Guest memory did not read, or an SSE exception is unmasked.
    Unread,
}

/// A bitmap of the HUD's interface at `field`, as `0x002544b4`..
/// `0x002544d6` finds it: the first bitmap of its tag.
fn blip_bitmap(mem: &dyn GuestMemory, interface: u32, field: u32) -> Result<u32, BlipRefusal> {
    let read = |at: u32| mem.u32(at).map_err(|_| BlipRefusal::Unread);
    let index = read(interface.wrapping_add(field))? & 0xffff;
    let definition = read(read(TAG_INSTANCES)?.wrapping_add(index << 4).wrapping_add(8))?;
    if definition == 0 || read(definition.wrapping_add(0x44))? as i32 <= 0 {
        return Err(BlipRefusal::Bitmap);
    }
    match read(definition.wrapping_add(0x48))? {
        0 => Err(BlipRefusal::Bitmap),
        bitmap => Ok(bitmap),
    }
}

/// `0x00012360` and `0x00012310` with no call: a bitmap whose `+0x70` is
/// past the cache's stamp gives its texture `+0x50` at once.
fn resident(mem: &dyn GuestMemory, bitmap: u32) -> Result<u32, BlipRefusal> {
    let read = |at: u32| mem.u32(at).map_err(|_| BlipRefusal::Unread);
    let stamp = read(bitmap.wrapping_add(0x70))? as i32;
    if stamp <= read(CACHE_STAMP)? as i32 {
        return Err(BlipRefusal::Bitmap);
    }
    match read(bitmap.wrapping_add(0x50))? {
        0 => Err(BlipRefusal::Bitmap),
        texture => Ok(texture),
    }
}

/// The draw of the blip whose call's stack is at `esp` (the return
/// address at `[esp]`, the five arguments after it).
pub fn blip_draw(mem: &dyn GuestMemory, esp: u32, mxcsr: u32) -> Result<BlipDraw, BlipRefusal> {
    let read = |at: u32| mem.u32(at).map_err(|_| BlipRefusal::Unread);
    let arg = |n: u32| read(esp.wrapping_add(4 + 4 * n));
    let globals = read(HUD_GLOBALS)?;
    let interface = if read(globals.wrapping_add(0x110))? != 0 { read(globals.wrapping_add(0x114))? } else { 0 };
    if interface == 0 {
        return Err(BlipRefusal::Bitmap);
    }
    let first = blip_bitmap(mem, interface, 0x64)?;
    let second = blip_bitmap(mem, interface, 0x6c)?;
    resident(mem, first)?;
    resident(mem, second)?;
    let bitmap = if arg(4)? as u8 != 0 { second } else { first };
    let texture = resident(mem, bitmap)?;
    let short = |at: u32| mem.u16(at).map(|v| i32::from(v as i16)).map_err(|_| BlipRefusal::Unread);
    let (width, height) = (short(bitmap.wrapping_add(4))?, short(bitmap.wrapping_add(6))?);
    let draws = mem.u8(BLIPS_DRAW).map_err(|_| BlipRefusal::Unread)? != 0;

    let at = arg(0)?;
    let (px, py) = (read(at)?, read(at.wrapping_add(4))?);
    let (shade, size, colour) = (arg(1)?, arg(2)?, arg(3)?);
    let (alpha, depth) = (read(ONE)?, read(DEPTH)?);
    let mut sse = Sse { mxcsr };
    let mut ops = || -> Result<_, Error> {
        let half = sse.mul(size, mem.u32(HALF)?)?;
        let b = sse.mul(mem.u32(colour.wrapping_add(8))?, shade)?;
        let g = sse.mul(mem.u32(colour.wrapping_add(4))?, shade)?;
        let r = sse.mul(mem.u32(colour)?, shade)?;
        let left = sse.sub(px, half)?;
        let top = sse.add(py, half)?;
        let right = sse.add(px, half)?;
        let bottom = sse.sub(py, half)?;
        Ok(([r, g, b, alpha], [left, top, right, bottom]))
    };
    let (rgba, [left, top, right, bottom]) = ops().map_err(|_| BlipRefusal::Unread)?;
    let f = f32::from_bits;
    let colour = (5u8, rgba.map(f));
    let corner = |u: f32, v: f32, x: u32, y: u32| [colour, (1u8, [u, v, 0.0, 1.0]), (0u8, [f(x), f(y), f(depth), f(depth)])];
    Ok(BlipDraw {
        draws,
        texture,
        width,
        height,
        vertices: [
            corner(0.0, 0.0, left, top),
            corner(1.0, 0.0, right, top),
            corner(1.0, 1.0, right, bottom),
            corner(0.0, 1.0, left, bottom),
        ],
        mxcsr: sse.mxcsr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    const MXCSR: u32 = 0x1f80;
    const CONTROL: u16 = 0x027f;
    const GLOBALS: u32 = 0x0060_0000;
    const TIME_AT: u32 = 0x0061_0000;

    /// The retail constants, a 640 x 480 window at (0, 0) whose safe
    /// rectangle is inset by 24 and 32, the retail centres, and user 0
    /// in first person.
    fn image() -> FakeGuest {
        let mut m = FakeGuest::new();
        m.map(0x0041_0000, 0x0008_0000);
        function_eval::put_constants(&mut m);
        m.put_f32(STEP, 0.1).put_f32(SWELL, 7.0).put_f32(ONE, 1.0);
        m.put_f32(PI, std::f32::consts::PI).put_f32(PULSE, 1.0 / 30.0).put_f32(BLEND_FIRST_PERSON, 0.0);
        m.write(EXPONENT, &3.5f64.to_bits().to_le_bytes()).unwrap();
        for (i, v) in [0u16, 0, 480, 640].into_iter().enumerate() {
            m.put_u16(WINDOW + 2 * i as u32, v);
        }
        for (i, v) in [24u16, 32, 456, 608].into_iter().enumerate() {
            m.put_u16(SAFE + 2 * i as u32, v);
        }
        for (i, [x, y]) in [[41i16, -42], [31, -31], [31, -31]].into_iter().enumerate() {
            m.put_u16(CENTRES + 4 * i as u32, x as u16).put_u16(CENTRES + 4 * i as u32 + 2, y as u16);
        }
        m.put_u32(CAMERAS + 0xc, CAMERA_FIRST_PERSON).put_f32(CAMERAS + 4, 0.0);
        m.put_u32(SENSOR, GLOBALS).put_u32(GLOBALS + RING_START, 3);
        for f in 0..FRAMES as u32 {
            m.put_u32(GLOBALS + f * FRAME_LEN + FRAME_COUNT, 0);
        }
        for (i, v) in [0.0f32, 2.0, 3.0].into_iter().enumerate() {
            m.put_f32(SIZES + 4 * i as u32, v);
        }
        m.put_u32(TIME, TIME_AT).put_f32(TIME_AT + 4, 1.0 / 30.0).put_u32(TIME_AT + 8, 45);
        m.put_f32(COMPOSITE_VALUE, 0.75);
        m
    }

    fn put_blip(m: &mut FakeGuest, frame: u32, index: u32, blip: [u8; 4]) {
        let at = GLOBALS + frame * FRAME_LEN;
        m.write(at + 4 * index, &blip).unwrap();
        let count = m.i32(at + FRAME_COUNT).unwrap().max(index as i32 + 1);
        m.put_u32(at + FRAME_COUNT, count as u32);
    }

    #[test]
    fn the_sensor_sits_at_the_modes_centre_from_the_safe_bottom_left() {
        let mut m = image();
        assert_eq!(screen_mode(&m), Ok(0));
        assert_eq!(point(&m, MXCSR), Ok([32 + 41, 456 - 42]));
        // A half-height window is mode 1, a quarter mode 2.
        m.put_u16(WINDOW + 4, 240);
        assert_eq!(screen_mode(&m), Ok(1));
        assert_eq!(point(&m, MXCSR), Ok([32 + 31, 456 - 31]));
        m.put_u16(WINDOW + 6, 320);
        assert_eq!(screen_mode(&m), Ok(2));
        // The window's corner is taken off, and put back for the centre.
        m.put_u16(WINDOW + 2, 320).put_u16(WINDOW + 6, 640);
        let p = point(&m, MXCSR).unwrap();
        assert_eq!(p, [32 + 31 - 320, 456 - 31]);
        let sensor = draw(&m, MXCSR, CONTROL, 0, p).unwrap().unwrap();
        assert_eq!(sensor.centre, [63.0f32.to_bits(), 425.0f32.to_bits()]);
    }

    #[test]
    fn a_dead_or_third_person_camera_draws_no_sensor() {
        let mut m = image();
        assert_eq!(camera_mode(&m, MXCSR, 0), Ok(0));
        m.put_f32(CAMERAS + 4, 0.5);
        assert_eq!(camera_mode(&m, MXCSR, 0), Ok(3));
        assert_eq!(draw(&m, MXCSR, CONTROL, 0, [0, 0]), Ok(None));
        m.put_u32(CAMERAS + 0xc, CAMERA_DEAD);
        assert_eq!(draw(&m, MXCSR, CONTROL, 0, [0, 0]), Ok(None));
        m.put_u32(CAMERAS + 0xc, CAMERA_FOLLOWING);
        assert_eq!(camera_mode(&m, MXCSR, 0), Ok(1));
        assert!(draw(&m, MXCSR, CONTROL, 0, [0, 0]).unwrap().is_some());
        m.put_u32(CAMERAS + 0xc, CAMERA_FLYING).put_u8(FLYING_MODE, 0);
        assert_eq!(camera_mode(&m, MXCSR, 0), Ok(3));
        m.put_u8(FLYING_MODE, 1);
        assert_eq!(camera_mode(&m, MXCSR, 0), Ok(2));
    }

    #[test]
    fn the_blips_draw_from_the_rings_start_each_shaded_and_swollen_by_its_step() {
        let mut m = image();
        // The ring starts at frame 3: step 0 is frame 3 (n = 10), step 9
        // frame 2 (n = 1). Type 0 is no blip.
        put_blip(&mut m, 3, 0, [5, 0xfb, 1, 1]);
        put_blip(&mut m, 3, 1, [0, 0, 0, 2]);
        put_blip(&mut m, 2, 0, [0xf6, 10, 2, 2]);
        let sensor = draw(&m, MXCSR, CONTROL, 0, [73, 414]).unwrap().unwrap();
        assert_eq!(sensor.centre, [73.0f32.to_bits(), 414.0f32.to_bits()]);
        assert_eq!(sensor.value, 0.75f32.to_bits());
        let [first, last] = sensor.blips[..] else { panic!("{:?}", sensor.blips) };
        assert_eq!(first.position, [5.0f32.to_bits(), (-5.0f32).to_bits()]);
        assert_eq!(last.position, [(-10.0f32).to_bits(), 10.0f32.to_bits()]);
        assert_eq!((first.colour, last.colour), (COLOURS + 12, COLOURS + 24));
        assert!(!first.pulse && !last.pulse);
        // n = 10: a = 1, so the shade is 1 and the swell pow(0) * 7 + 1.
        assert_eq!(f32::from_bits(first.shade), 1.0);
        assert_eq!(f32::from_bits(first.size), 1.0 + 2.0);
        // n = 1: a = 0.1, the shade 0.01, the swell 0.9^3.5 * 7 + 1.
        let a = 0.1f32;
        assert_eq!(last.shade, (a * a).to_bits());
        let swell = ((1.0 - f64::from(a)).powf(3.5) * 7.0 + 1.0) as f32;
        assert!((f32::from_bits(last.size) - (swell + 3.0)).abs() < 1e-5, "{}", f32::from_bits(last.size));
    }

    #[test]
    fn a_type_6_blip_pulses_with_the_game_clock() {
        let mut m = image();
        put_blip(&mut m, 3, 0, [0, 0, PULSE_TYPE, 0]);
        let sensor = draw(&m, MXCSR, CONTROL, 0, [0, 0]).unwrap().unwrap();
        let [blip] = sensor.blips[..] else { panic!() };
        assert!(blip.pulse);
        assert_eq!(blip.colour, COLOURS + 72);
        // 45 ticks at 1/30 s is 1.5 s: sin(1.5 pi) = -1, so the base is 1.
        assert!((f32::from_bits(blip.size) - 1.0).abs() < 1e-5, "{}", f32::from_bits(blip.size));
        m.put_u32(TIME_AT + 8, 15);
        let sensor = draw(&m, MXCSR, CONTROL, 0, [0, 0]).unwrap().unwrap();
        let want = 2.0f32 / 30.0 + 1.0;
        assert!((f32::from_bits(sensor.blips[0].size) - want).abs() < 1e-5);
    }

    #[test]
    fn another_users_blips_are_their_own_and_a_runaway_count_stops() {
        let mut m = image();
        m.put_u32(CAMERAS + CAMERA_LEN + 0xc, CAMERA_FIRST_PERSON).put_f32(CAMERAS + CAMERA_LEN + 4, 0.0);
        put_blip(&mut m, 3, 0, [1, 1, 1, 0]);
        assert_eq!(draw(&m, MXCSR, CONTROL, 1, [0, 0]).unwrap().unwrap().blips, []);
        m.put_u32(GLOBALS + 3 * FRAME_LEN + FRAME_COUNT, (MOST_BLIPS + 1) as u32);
        assert_eq!(draw(&m, MXCSR, CONTROL, 0, [0, 0]), Err(Error::Unbounded));
    }

    /// A blip image: the HUD's interface naming two resident bitmaps
    /// (textures `0x7100` and `0x7200`, 16 by 8 and 32 by -2), the
    /// retail constants, the blips allowed to draw, and a call's stack at
    /// `ESP` for a blip at (10, 20) of size 4, shade 0.5, colour
    /// (0.5, 0.25, 1).
    const ESP: u32 = 0x0070_0000;
    const INTERFACE: u32 = 0x0071_0000;
    const BITMAPS: [u32; 2] = [0x0072_0000, 0x0072_1000];

    fn blip_image(pulse: bool) -> FakeGuest {
        let mut m = FakeGuest::new();
        m.put_u32(HUD_GLOBALS, 0x0073_0000).put_u32(0x0073_0110, 1).put_u32(0x0073_0114, INTERFACE);
        m.put_u32(INTERFACE + 0x64, 0xe001_0002).put_u32(INTERFACE + 0x6c, 0xe002_0003);
        m.put_u32(TAG_INSTANCES, 0x0074_0000).put_u32(CACHE_STAMP, 100);
        for (index, (definition, bitmap)) in [(2u32, (0x0075_0000u32, BITMAPS[0])), (3, (0x0075_1000, BITMAPS[1]))] {
            m.put_u32(0x0074_0000 + 16 * index + 8, definition);
            m.put_u32(definition + 0x44, 1).put_u32(definition + 0x48, bitmap);
        }
        for (bitmap, texture, [w, h]) in [(BITMAPS[0], 0x7100, [16u16, 8]), (BITMAPS[1], 0x7200, [32, 0xfffe])] {
            m.put_u32(bitmap + 0x70, 101).put_u32(bitmap + 0x50, texture);
            m.put_u16(bitmap + 4, w).put_u16(bitmap + 6, h);
        }
        m.put_u8(BLIPS_DRAW, 1).put_f32(ONE, 1.0).put_f32(HALF, 0.5).put_f32(DEPTH, 16_777_215.0);
        m.put_u32(ESP, 0x001a_2219).put_u32(ESP + 4, 0x0076_0000).put_f32(ESP + 8, 0.5).put_f32(ESP + 12, 4.0);
        m.put_u32(ESP + 16, 0x0077_0000).put_u32(ESP + 20, u32::from(pulse) | 0xab00);
        m.put_f32(0x0076_0000, 10.0).put_f32(0x0076_0004, 20.0);
        m.put_f32(0x0077_0000, 0.5).put_f32(0x0077_0004, 0.25).put_f32(0x0077_0008, 1.0);
        m
    }

    #[test]
    fn a_blips_quad_is_its_bodys_corners_colour_and_texture() {
        let m = blip_image(false);
        let d = blip_draw(&m, ESP, MXCSR).unwrap();
        assert!(d.draws);
        assert_eq!((d.texture, d.width, d.height), (0x7100, 16, 8));
        let colour = (5, [0.25, 0.125, 0.5, 1.0]);
        let z = 16_777_215.0;
        // Half the size about the position, y up first, in the body's order.
        assert_eq!(d.vertices[0], [colour, (1, [0.0, 0.0, 0.0, 1.0]), (0, [8.0, 22.0, z, z])]);
        assert_eq!(d.vertices[1], [colour, (1, [1.0, 0.0, 0.0, 1.0]), (0, [12.0, 22.0, z, z])]);
        assert_eq!(d.vertices[2], [colour, (1, [1.0, 1.0, 0.0, 1.0]), (0, [12.0, 18.0, z, z])]);
        assert_eq!(d.vertices[3], [colour, (1, [0.0, 1.0, 0.0, 1.0]), (0, [8.0, 18.0, z, z])]);
    }

    #[test]
    fn a_pulsing_blip_takes_the_second_bitmap_widened_and_a_held_flag_draws_nothing() {
        let mut m = blip_image(true);
        let d = blip_draw(&m, ESP, MXCSR).unwrap();
        assert_eq!((d.texture, d.width, d.height), (0x7200, 32, -2));
        m.put_u8(BLIPS_DRAW, 0);
        assert!(!blip_draw(&m, ESP, MXCSR).unwrap().draws);
    }

    #[test]
    fn a_bitmap_the_cache_must_load_is_refused() {
        // The stamp not past the cache's: the body calls the cache.
        let mut m = blip_image(false);
        m.put_u32(BITMAPS[1] + 0x70, 100);
        assert_eq!(blip_draw(&m, ESP, MXCSR), Err(BlipRefusal::Bitmap));
        let mut m = blip_image(false);
        m.put_u32(BITMAPS[0] + 0x50, 0);
        assert_eq!(blip_draw(&m, ESP, MXCSR), Err(BlipRefusal::Bitmap));
        let mut m = blip_image(false);
        m.put_u32(0x0073_0110, 0);
        assert_eq!(blip_draw(&m, ESP, MXCSR), Err(BlipRefusal::Bitmap));
    }
}
