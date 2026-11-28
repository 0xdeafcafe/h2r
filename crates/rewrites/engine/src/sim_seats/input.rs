//! The phantom action build's input stage (docs/perf/sim-phantom-build.md,
//! "The input stage in Rust"): the four guest calls around the camera
//! origin that turn a seat's pad and history into the records the action
//! builder `0x186ab0` reads.
//!
//! | Piece | Entry | Reads | Writes |
//! |---|---|---|---|
//! | [`bind`] | `0x1872d0` | the control globals, the unit | the user's control block |
//! | [`look_rates`] | `0x190c34` (and `0x218420`) | the user's profile | the look-rate record `0x51ea18` |
//! | [`read_pads`] | `0x124c00` | the pad report, the raw history | raw input `0x4e61dc` |
//! | [`remap`] | `0x217c40` | raw input, look rates | abstract input `0x51ea88`, latch `0x51ebcc` |
//!
//! Each piece reads guest RAM through [`Memory`], computes every byte the
//! original stores, and writes them only when the whole piece has run, so
//! a [`Refusal`] leaves RAM as it was and the caller runs the guest entry
//! instead. The float environment is the call's own: the x87 state of the
//! registers the guest call would start with ([`load_fpu`]), and SSE
//! arithmetic as native `f32` under the one MXCSR that makes them the same
//! ([`Refusal::Mxcsr`]). Locals on the guest stack are not written.
use crate::light_constants::{Error, X87};
use crate::sim_native::Memory;
use crate::sim_physics::spatial::load_fpu;
use runtime::{CpuAbi, GuestFault};
use x87::abi::{arith_op, unary_op};

pub const BIND: u32 = 0x0018_72d0;
pub const LOOK_RATES: u32 = 0x0019_0c34;
pub const READ_PADS: u32 = 0x0012_4c00;
pub const REMAP: u32 = 0x0021_7c40;
/// The remap's look-inversion helper, called when a profile's `+0x1b`
/// bit is set and its `+0x1a` bit is not; [`Refusal::LookHelper`].
pub const LOOK_HELPER: u32 = 0x0021_8510;

/// The four ports' XInput handles; zero is no pad.
pub const HANDLES: u32 = 0x004e_61cc;
/// Raw input, one 0x48-byte record a port.
pub const RAW: u32 = 0x004e_61dc;
pub const RAW_SIZE: u32 = 0x48;
/// Set: the remap reads the replay record [`REPLAY_RAW`] for every port.
pub const REPLAY_SELECT: u32 = 0x004e_61b9;
pub const REPLAY_RAW: u32 = 0x004e_630c;
/// Look rates, one 0x1c-byte record a user.
pub const RATES: u32 = 0x0051_ea18;
pub const RATES_SIZE: u32 = 0x1c;
/// Abstract input, one 0x50-byte record a port, and a byte latch a port.
pub const LAYOUT: u32 = 0x0051_ea88;
pub const LAYOUT_SIZE: u32 = 0x50;
pub const LATCH: u32 = 0x0051_ebcc;
/// Profiles, 0xc70 bytes a user; the input stage reads `+0..7`.
pub const PROFILES: u32 = 0x0054_e9f4;
pub const PROFILE_STRIDE: u32 = 0xc70;
const CONTROL_GLOBALS: u32 = 0x004e_d284;
pub const CONTROL_SIZE: u32 = 0x94;
const OBJECT_HEADER: u32 = 0x004e_0300;
const PORTS: u32 = 4;

/// The `XINPUT_STATE` the phantom executor's `XInputGetState` answers for
/// every port: the packet number, the buttons word, eight analog bytes and
/// four thumb axes.
pub const REPORT: usize = 0x16;

// Read-only constants, read from the image as the original does.
const ANALOG_ORDER: u32 = 0x0044_0c38;
const DIGITAL_MASKS: u32 = 0x0044_0c40;
const SENSITIVITY_RATES: [u32; 2] = [0x0044_4c04, 0x0044_4bdc];
const DEFAULT_RATES: [u32; 2] = [0x0044_a038, 0x0044_a03c];
const PITCH_LIMITS: [u32; 2] = [0x0045_e054, 0x0045_e050];
const TWO_PI: u32 = 0x0043_f17c;
const STICK_SCALE: u32 = 0x0043_ba70;
const ONE: u32 = 0x0045_dbc0;
const MINUS_ONE: u32 = 0x0045_dbcc;
const TRIGGER_SCALE: u32 = 0x0045_dbb4;
const QUADRANTS: u32 = 0x0045_70a0;
const RIGHT_DIAGONAL: u32 = 0x0045_e010;
const SLOPE: u32 = 0x0045_e014;
const STEEP: u32 = 0x0045_e018;
const SHALLOW: u32 = 0x0045_e01c;
const LEFT_DIAGONAL: u32 = 0x0045_e020;

/// Why a piece did not run. Nothing was written; the caller runs the
/// guest entry for that piece.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Refusal {
    /// MXCSR other than round to nearest with every exception masked and
    /// no flush or denormals-are-zero: the SSE steps are native `f32`.
    Mxcsr,
    /// An x87 step the x87 crate stops on (a transcendental under a
    /// directed rounding mode) or an unmasked exception.
    X87,
    /// `0x4e61b9` set: the remap reads the replay record for every port.
    ReplayRecord,
    /// A profile whose remap calls [`LOOK_HELPER`].
    LookHelper,
    /// A button order or layout slot past the report or the 16 buttons:
    /// the original reads its own stack or the next record.
    Slot,
    /// A guest read or write faulted.
    Memory,
}

impl Refusal {
    /// The name a build reports the refusal by.
    pub fn name(self) -> &'static str {
        match self {
            Self::Mxcsr => "input.mxcsr",
            Self::X87 => "input.x87",
            Self::ReplayRecord => "input.replay-record",
            Self::LookHelper => "input.look-helper",
            Self::Slot => "input.slot",
            Self::Memory => "input.memory",
        }
    }
}

impl From<GuestFault> for Refusal {
    fn from(_: GuestFault) -> Self {
        Self::Memory
    }
}

impl From<Error> for Refusal {
    fn from(e: Error) -> Self {
        match e {
            Error::Memory(_) => Self::Memory,
            _ => Self::X87,
        }
    }
}

type Result<T = ()> = std::result::Result<T, Refusal>;

fn word(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn half(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

fn get_half(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn f(m: &impl Memory, at: u32) -> Result<f32> {
    Ok(f32::from_bits(m.u32(at)?))
}

/// The SSE steps are native `f32` only under the reset MXCSR's controls.
fn sse_native(cpu: &CpuAbi) -> Result {
    if cpu.mxcsr & 0xffc0 == 0x1f80 { Ok(()) } else { Err(Refusal::Mxcsr) }
}

/// `ja` after `comiss`/`fcomip`: above and ordered.
fn above(flags: u32) -> bool {
    flags & 0x41 == 0
}

fn fst_m64(x: &mut X87) -> Result<u64> {
    let (code, store) = x87::abi::st_f64(&x.0, 0);
    X87::ok(code)?;
    x87::abi::store_commit(&mut x.0, store.status, false);
    Ok(store.value)
}

fn fld_m64(x: &mut X87, bits: u64) -> Result {
    Ok(X87::ok(x87::abi::ld_f64(&mut x.0, bits))?)
}

/// `0x1872d0`, user in `eax`, unit in `ecx`: nothing when the control
/// globals are active and the user's block already holds the unit;
/// otherwise the block is reset to the unit, its facing seeded from the
/// unit's forward vector (two `fpatan`s), and three unit fields copied.
pub fn bind(m: &mut impl Memory, cpu: &CpuAbi, user: u32, unit: u32) -> Result {
    if user == u32::MAX {
        return Ok(());
    }
    let globals = m.u32(CONTROL_GLOBALS)?;
    let block = globals.wrapping_add(user.wrapping_mul(CONTROL_SIZE)).wrapping_add(0x14);
    if m.u8(globals)? != 0 && m.u32(block)? == unit {
        return Ok(());
    }
    sse_native(cpu)?;
    let mut b = [0u8; CONTROL_SIZE as usize];
    word(&mut b, 0x84, m.u32(PITCH_LIMITS[0])?);
    word(&mut b, 0x80, m.u32(PITCH_LIMITS[1])?);
    word(&mut b, 0, unit);
    half(&mut b, 0x28, u16::MAX);
    b[0x2a] = 0xff;
    b[0x2b] = 0xff;
    half(&mut b, 0x2c, u16::MAX);
    half(&mut b, 0x2e, u16::MAX);
    for at in [0x44, 0x48, 0x4c, 0x70, 0x74] {
        word(&mut b, at, u32::MAX);
    }
    if unit != u32::MAX {
        let table = m.u32(m.u32(OBJECT_HEADER)?.wrapping_add(0x44))?;
        let object = m.u32(table.wrapping_add((unit & 0xffff) * 12).wrapping_add(8))?;
        let at = |o: u32| object.wrapping_add(o);
        let (x, y, z) = (m.u32(at(0x150))?, m.u32(at(0x154))?, m.u32(at(0x158))?);
        let mut fpu = load_fpu(cpu);
        fpu.fld(y)?;
        fpu.fld(x)?;
        fpu.unary(unary_op::PATAN)?;
        let mut yaw = fpu.fstp_m32()?;
        fpu.fld(y)?;
        fpu.fld(x)?;
        fpu.fld(z)?;
        fpu.fld_st(2)?;
        fpu.arith_reg(arith_op::MUL, 3, 0, true)?;
        fpu.fld_st(1)?;
        fpu.arith_reg(arith_op::MUL, 0, 2, false)?;
        fpu.arith_reg(arith_op::ADD, 3, 0, true)?;
        fpu.fxch(2)?;
        fpu.unary(unary_op::SQRT)?;
        fpu.fxch(2)?;
        fpu.fxch(1)?;
        fpu.fxch(2)?;
        fpu.unary(unary_op::PATAN)?;
        fpu.fstp_st(1)?;
        let pitch = fpu.fstp_m32()?;
        if 0.0 > f32::from_bits(yaw) {
            yaw = (f32::from_bits(yaw) + f(m, TWO_PI)?).to_bits();
        }
        word(&mut b, 0x10, yaw);
        word(&mut b, 0x14, pitch);
        word(&mut b, 0x28, m.u32(at(0x214))?);
        half(&mut b, 0x2c, m.u8(at(0x23d))? as i8 as u16);
        half(&mut b, 0x2e, m.u8(at(0x241))? as i8 as u16);
    }
    m.write_bytes(block, &b, 1)?;
    Ok(())
}

/// `0x190c34`, user in `eax`: the look-rate record from the profile's
/// first seven bytes over the Default layout `0x218420`. No float is
/// computed; the rates are table entries.
pub fn look_rates(m: &mut impl Memory, user: u32) -> Result {
    let p: [u8; 7] = m.load(PROFILES.wrapping_add(user.wrapping_mul(PROFILE_STRIDE)))?;
    let mut r = [0u8; RATES_SIZE as usize];
    let sensitivity = u32::from(p[6].clamp(1, 10) - 1);
    word(&mut r, 0, m.u32(SENSITIVITY_RATES[0] + 4 * sensitivity)?);
    word(&mut r, 4, m.u32(SENSITIVITY_RATES[1] + 4 * sensitivity)?);
    // Only to read them as the original does: both are overwritten above.
    let _ = (m.u32(DEFAULT_RATES[0])?, m.u32(DEFAULT_RATES[1])?);
    r[8..0x18].copy_from_slice(&[0, 4, 2, 3, 1, 5, 6, 7, 0xc, 0xd, 0xe, 0xf, 0xa, 0xb, 0, 1]);
    half(&mut r, 0x18, u16::from(p[5].min(3)));
    match p[4] {
        1 => (r[0xe], r[0xf]) = (7, 6),
        2 => (r[0xc], r[0xe]) = (6, 1),
        3 => (r[0xc], r[0x13]) = (0xf, 1),
        _ => {}
    }
    r[0x1a] = p[0] & 1;
    r[0x1b] = (u32::from_le_bytes([p[0], p[1], p[2], p[3]]) >> 2) as u8 & 1;
    m.write_bytes(RATES.wrapping_add(user.wrapping_mul(RATES_SIZE)), &r, 1)?;
    Ok(())
}

/// `0x124c00(elapsed)`: for each port with a handle, the pad `report`
/// (the phantom executor answers every port's `XInputGetState` with the
/// same one, and success) folded into the port's raw history: down
/// counts, held milliseconds and press thresholds for eight analog and
/// eight digital buttons, and the four thumb axes with their dead zone.
/// Integers only.
pub fn read_pads(m: &mut impl Memory, elapsed: u32, report: &[u8; REPORT]) -> Result {
    let order: [u8; 8] = m.load(ANALOG_ORDER)?;
    let masks: [u8; 8] = m.load(DIGITAL_MASKS)?;
    if order.iter().any(|&i| 6 + usize::from(i) >= REPORT) {
        return Err(Refusal::Slot);
    }
    let mut out = Vec::new();
    for port in 0..PORTS {
        if m.u32(HANDLES + 4 * port)? == 0 {
            continue;
        }
        let at = RAW + RAW_SIZE * port;
        let mut raw: [u8; RAW_SIZE as usize] = m.load(at)?;
        read_pad(&mut raw, report, elapsed, &order, &masks);
        out.push((at, raw));
    }
    for (at, raw) in out {
        m.write_bytes(at, &raw, 1)?;
    }
    Ok(())
}

fn held(pressed: bool, count: u8, ms: u16, elapsed: u32) -> (u8, u16) {
    if !pressed {
        return (0, 0);
    }
    let sum = u32::from(ms).wrapping_add(elapsed);
    (count.saturating_add(1), if sum as i32 <= 0xffff { sum as u16 } else { 0xffff })
}

/// One port of [`read_pads`] on its raw record.
pub fn read_pad(raw: &mut [u8; RAW_SIZE as usize], report: &[u8; REPORT], elapsed: u32, order: &[u8; 8], masks: &[u8; 8]) {
    for i in 0..8 {
        let threshold = raw[8 + i];
        let pressed = raw[i] > threshold;
        let value = report[6 + usize::from(order[i])];
        raw[i] = value;
        let (count, ms) = held(pressed, raw[16 + i], get_half(raw, 0x20 + 2 * i), elapsed);
        raw[16 + i] = count;
        half(raw, 0x20 + 2 * i, ms);
        if pressed {
            let t = value.saturating_sub(0x20);
            if t > threshold {
                raw[8 + i] = t;
            }
        } else {
            let t = value.saturating_add(0x40);
            if t < threshold {
                raw[8 + i] = t;
            }
        }
    }
    let buttons = get_half(report, 4);
    for i in 0..8 {
        let pressed = u16::from(masks[i]) & buttons != 0;
        let (count, ms) = held(pressed, raw[0x18 + i], get_half(raw, 0x30 + 2 * i), elapsed);
        raw[0x18 + i] = count;
        half(raw, 0x30 + 2 * i, ms);
    }
    for j in 0..4 {
        let v = get_half(report, 0xe + 2 * j) as i16;
        half(raw, 0x40 + 2 * j, axis(v));
    }
}

/// A thumb axis past its 9000 dead zone, rescaled to the full range: the
/// original's reciprocal multiplies, bit for bit.
fn axis(v: i16) -> u16 {
    let v = i32::from(v);
    let high = |x: u32, magic: i64| ((i64::from(x as i32) * magic) >> 32) as i32;
    let mut out = 0i32;
    if v > 9000 {
        let x = ((v - 9000) as u32).wrapping_mul(0x7fff);
        let d = high(x, 0x0583_cec1) >> 9;
        out = d.wrapping_add((d as u32 >> 31) as i32);
    }
    if v < -9000 {
        let x = ((-9000 - v) as u32) << 15;
        let d = high(x, 0x4f88_0e79).wrapping_sub(x as i32) >> 14;
        out = d.wrapping_add((d as u32 >> 31) as i32);
    }
    out as u16
}

/// The remap's read-only constants.
struct Constants {
    scale: f32,
    one: f32,
    min: f32,
    trigger: f32,
    quadrants: [u32; 4],
    left_diagonal: u32,
    right_diagonal: u32,
    slope: f32,
    steep: f32,
    shallow: u32,
}

impl Constants {
    fn read(m: &impl Memory) -> Result<Self> {
        Ok(Self {
            scale: f(m, STICK_SCALE)?,
            one: f(m, ONE)?,
            min: f(m, MINUS_ONE)?,
            trigger: f(m, TRIGGER_SCALE)?,
            quadrants: [m.u32(QUADRANTS)?, m.u32(QUADRANTS + 4)?, m.u32(QUADRANTS + 8)?, m.u32(QUADRANTS + 12)?],
            left_diagonal: m.u32(LEFT_DIAGONAL)?,
            right_diagonal: m.u32(RIGHT_DIAGONAL)?,
            slope: f(m, SLOPE)?,
            steep: f(m, STEEP)?,
            shallow: m.u32(SHALLOW)?,
        })
    }

    /// `comiss min, x; jbe` then `comiss x, one; jbe`.
    fn clamp(&self, x: f32) -> f32 {
        if self.min > x {
            self.min
        } else if x > self.one {
            self.one
        } else {
            x
        }
    }
}

/// `comiss zero, x`, then `cvtsi2ss` of -1 or 1.
fn sign(x: f32) -> f32 {
    if 0.0 > x { -1.0 } else { 1.0 }
}

/// `0x217c40`: for each port with a handle, raw input to abstract input
/// through the user's look rates, and the port's latch set; a port with no
/// handle has its latch cleared.
pub fn remap(m: &mut impl Memory, cpu: &CpuAbi) -> Result {
    sse_native(cpu)?;
    let k = Constants::read(m)?;
    let mut fpu = load_fpu(cpu);
    let mut layouts = Vec::new();
    let mut latches = [0u8; PORTS as usize];
    for port in 0..PORTS {
        let latch = LATCH + port;
        if m.u32(HANDLES + 4 * port)? == 0 {
            latches[port as usize] = 0;
            continue;
        }
        if m.u8(REPLAY_SELECT)? != 0 {
            return Err(Refusal::ReplayRecord);
        }
        let raw: [u8; RAW_SIZE as usize] = m.load(RAW + RAW_SIZE * port)?;
        let rates: [u8; RATES_SIZE as usize] = m.load(RATES + RATES_SIZE * port)?;
        let at = LAYOUT + LAYOUT_SIZE * port;
        let mut layout: [u8; LAYOUT_SIZE as usize] = m.load(at)?;
        remap_port(&mut fpu, &k, &raw, &rates, &mut layout)?;
        layouts.push((at, layout));
        latches[port as usize] = match m.u8(latch)? {
            0 => 1,
            other => other,
        };
    }
    for (at, layout) in layouts {
        m.write_bytes(at, &layout, 1)?;
    }
    m.write_bytes(LATCH, &latches, 1)?;
    Ok(())
}

/// A stick's angle (as an `f32`, and for the left stick also as the
/// double the original keeps), the larger of its |sine| and |cosine| (each
/// as an `f32`, compared against the unrounded cosine) and the stick
/// normalised by it: `0x217ca5`..`0x217d6d` and `0x217d6d`..`0x217e28`.
/// The right stick's angle (`keep`) is left on the x87 stack.
fn stick(fpu: &mut X87, k: &Constants, x: i16, y: i16, keep: bool) -> Result<(u32, u64, f32, f32)> {
    fpu.fild(i32::from(y))?;
    let fy = fpu.fstp_m32()?;
    fpu.fild(i32::from(x))?;
    let fx = fpu.fstp_m32()?;
    fpu.fld(fy)?;
    fpu.fld(fx)?;
    fpu.unary(unary_op::PATAN)?;
    let wide = if keep { 0 } else { fst_m64(fpu)? };
    let angle = fpu.fst_m32()?;
    fpu.fld_st(0)?;
    fpu.fsin()?;
    fpu.unary(unary_op::ABS)?;
    let sine = fpu.fstp_m32()?;
    if keep {
        fpu.fld_st(0)?;
    }
    fpu.fcos()?;
    fpu.unary(unary_op::ABS)?;
    let cosine = fpu.fst_m32()?;
    fpu.fld(sine)?;
    let flags = fpu.fcomip(1)?;
    fpu.fstp_st(0)?;
    let largest = f32::from_bits(if above(flags) { sine } else { cosine });
    let inverse = k.one / largest;
    let x = k.clamp(f32::from_bits(fx) * inverse * k.scale);
    let y = k.clamp(f32::from_bits(fy) * inverse * k.scale);
    Ok((angle, wide, x, y))
}

/// `sqrt(x * x + y * y)` on the x87, `fld`/`fmul` y first.
fn length(fpu: &mut X87, x: f32, y: f32) -> Result<f32> {
    fpu.fld(y.to_bits())?;
    fpu.fmul_m(y.to_bits())?;
    fpu.fld(x.to_bits())?;
    fpu.fmul_m(x.to_bits())?;
    fpu.arith_reg(arith_op::ADD, 1, 0, true)?;
    fpu.unary(unary_op::SQRT)?;
    Ok(f32::from_bits(fpu.fstp_m32()?))
}

/// `fld a; fabs; fld b; fabs; fxch; fcomip; fstp st0`: |a| above |b|.
fn wider(fpu: &mut X87, a: f32, b: f32) -> Result<bool> {
    fpu.fld(a.to_bits())?;
    fpu.unary(unary_op::ABS)?;
    fpu.fld(b.to_bits())?;
    fpu.unary(unary_op::ABS)?;
    fpu.fxch(1)?;
    let flags = fpu.fcomip(1)?;
    fpu.fstp_st(0)?;
    Ok(above(flags))
}

/// `fld constant; fcomip st, st(1); fstp st0`: the constant above ST(0),
/// which is popped.
fn below(fpu: &mut X87, constant: u32) -> Result<bool> {
    fpu.fld(constant)?;
    let flags = fpu.fcomip(1)?;
    fpu.fstp_st(0)?;
    Ok(above(flags))
}

/// Modes 2 and 3 shape each stick by its angle's offset from its
/// quadrant's diagonal. Within the diagonal band (`near` is `Some`) one
/// axis keeps the stick's length and the other is bent by the offset:
/// `Some(true)` when |angle| is under 45 degrees or over 135 (`0x21806c`,
/// `0x2181e1`), `Some(false)` otherwise (`0x218006`, `0x21817b`). Outside
/// the band the stick snaps to its wider axis. Returns the new (x, y)
/// before the clamps, and whether the original skips the x clamp (the
/// snap to y).
fn shape(k: &Constants, x: f32, y: f32, length: f32, offset: f32, near: Option<bool>, x_wider: bool) -> (f32, f32, bool) {
    let bend = |s: f32| (k.one - offset * k.slope) * s * length;
    match near {
        Some(true) => (sign(x) * length, bend(sign(y)), false),
        Some(false) => (bend(sign(x)), sign(y) * length, false),
        None if x_wider => (sign(x) * length, 0.0, false),
        None => (0.0, sign(y) * length, true),
    }
}

fn remap_port(fpu: &mut X87, k: &Constants, raw: &[u8; 0x48], rates: &[u8; 0x1c], layout: &mut [u8; 0x50]) -> Result {
    let thumb = |at: usize| get_half(raw, at) as i16;
    let (left, left64, mut x, mut y) = stick(fpu, k, thumb(0x40), thumb(0x42), false)?;
    let (right, _, mut u, mut v) = stick(fpu, k, thumb(0x44), thumb(0x46), true)?;
    for j in 0..16 {
        let slot = usize::from(rates[8 + j]);
        if slot >= 16 {
            return Err(Refusal::Slot);
        }
        layout[j] = raw[0x10 + slot];
        half(layout, 0x10 + 2 * j, get_half(raw, 0x20 + 2 * slot));
    }
    word(layout, 0x30, (f32::from(raw[6]) * k.trigger).to_bits());
    word(layout, 0x34, (f32::from(raw[7]) * k.trigger).to_bits());
    let target = if rates[0xf] == 6 { 7 } else { 6 };
    layout[0x4e] = u8::from(rates[0xf] == 6);
    let slots = &rates[8..0x18];
    layout[0x4c] = slots.iter().position(|&s| s == target).map_or(6, |i| i as u8);
    layout[0x4d] = slots.iter().position(|&s| s == 1).map_or(4, |i| i as u8);
    let mode = get_half(rates, 0x18) as i16;
    if mode == 2 || mode == 3 {
        let quadrant = |a: f32, b: f32| usize::from(0.0 > a) | usize::from(0.0 > b) << 1;
        let (q1, q2) = (quadrant(x, y), quadrant(u, v));
        fpu.fld(left)?;
        fpu.arith_m(arith_op::SUB, k.quadrants[q1])?;
        let right_offset = f32::from_bits(right) - f32::from_bits(k.quadrants[q2]);
        let left_length = length(fpu, x, y)?;
        let right_length = length(fpu, u, v)?;
        fpu.unary(unary_op::ABS)?;
        let left_offset = f32::from_bits(fpu.fst_m32()?);
        // Stack: |left offset|, right angle.
        let left_near = if below(fpu, k.left_diagonal)? {
            fld_m64(fpu, left64)?;
            fpu.unary(unary_op::ABS)?;
            let a = f32::from_bits(fpu.fst_m32()?);
            Some(below(fpu, k.shallow)? || a > k.steep)
        } else {
            None
        };
        let x_wider = left_near.is_none() && wider(fpu, x, y)?;
        let (nx, ny, skip) = shape(k, x, y, left_length, left_offset, left_near, x_wider);
        (x, y) = (if skip { nx } else { k.clamp(nx) }, k.clamp(ny));
        // Stack: right angle.
        fpu.fld(right_offset.to_bits())?;
        fpu.unary(unary_op::ABS)?;
        let offset = f32::from_bits(fpu.fst_m32()?);
        let right_near = if below(fpu, k.right_diagonal)? {
            fpu.unary(unary_op::ABS)?;
            let a = f32::from_bits(fpu.fst_m32()?);
            Some(below(fpu, k.shallow)? || a > k.steep)
        } else {
            fpu.fstp_st(0)?;
            None
        };
        let u_wider = right_near.is_none() && wider(fpu, u, v)?;
        let (nu, nv, skip) = shape(k, u, v, right_length, offset, right_near, u_wider);
        (u, v) = (if skip { nu } else { k.clamp(nu) }, k.clamp(nv));
    } else {
        fpu.fstp_st(0)?;
    }
    if rates[0x1a] == 0 && rates[0x1b] != 0 {
        return Err(Refusal::LookHelper);
    }
    let factor = if rates[0x1a] != 0 { k.min } else { k.one };
    // The jump table `0x21840c`: modes 1 and 3 swap the sticks, and 3
    // also the two negated axes; any other mode takes mode 0's case.
    let (at_38, at_3c, at_40, kept) = match mode {
        1 => (v, 0.0 - u, 0.0 - x, y),
        3 => (v, 0.0 - x, 0.0 - u, y),
        _ => (y, 0.0 - x, 0.0 - u, v),
    };
    for (at, value) in [(0x38, at_38), (0x3c, at_3c), (0x40, at_40), (0x48, kept), (0x44, factor * kept)] {
        word(layout, at, value.to_bits());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_axis_dead_zone_is_the_originals_divide() {
        // The reciprocal multiplies are (v - 9000) * 32767 / 23767 and
        // (-9000 - v) * 32768 / -23768, truncated towards zero.
        for v in i16::MIN..=i16::MAX {
            let v32 = i32::from(v);
            let want = if v32 > 9000 {
                (v32 - 9000) * 0x7fff / 23767
            } else if v32 < -9000 {
                ((-9000 - v32) << 15) / -23768
            } else {
                0
            };
            assert_eq!(axis(v) as i16 as i32, want, "{v}");
        }
    }

    #[test]
    fn held_buttons_count_and_saturate_their_milliseconds() {
        assert_eq!(held(false, 9, 9, 33), (0, 0));
        assert_eq!(held(true, 254, 100, 33), (255, 133));
        assert_eq!(held(true, 255, 0xfff0, 33), (255, 0xffff));
        // The original compares the sum signed: a negative one is kept.
        assert_eq!(held(true, 0, 0x10, 0x8000_0000), (1, 0x0010));
    }
}
