//! The view setup's fog effect `0x00027aa0` in Rust, its callees
//! included (docs/graphics/retire-d3d-display.md, "The view setup and
//! view end"). Game code with no D3D call: it moves the effect's wind and
//! its four scrolling layers, and writes the layers' constants for the
//! fog's draw.
//!
//! The body (**Code** `0x00027aa0..0x0002857d`), when the context's effect
//! handle `[0x004857b8]` is not 0 or -1:
//!
//! 1. The record: index `[0x004b9a00]` (read, then replaced by the view's
//!    word `+0`, or 4 with its byte `+7`) picks `0x004b9a04 + 0x88 * n`.
//!    A fresh record (byte 0 clear) is zeroed, takes four random slots and
//!    the camera `0x0048568c`.
//! 2. The shared words `0x004b9d8c..0x004b9d9c` from the context's angle
//!    `+0x2c` (`fcos`) and the definition.
//! 3. The wind `0x00028580` ([`wind`]) on the record the new index picks,
//!    its 2D normalise `0x0002b400` and the CRT's `pow` `0x00372d48`.
//! 4. The camera step: `0x00142a60` ([`compose`]) of the camera with
//!    `*[0x004687d0]` (its position replaced by the definition's `+0x44`
//!    plus the wind) and `0x00141590` ([`inverse`]) of the record's last
//!    camera; the camera to the record.
//! 5. With far above near: the layers' angle (`fptan`, `fpatan`, `fcos`,
//!    `fsin`), the depth through `floor` `0x00321099` and `_ftol`
//!    `0x00322b0c`, a new random slot as it wraps, and for each of four
//!    layers at `0x004b9cac + 0x38 * i` its phase, weights (`pow`) and
//!    texture transform.
//!
//! Every write is staged and made at the end, in the body's order; a
//! refusal writes nothing. The frame's locals stay in Rust (nothing reads
//! the guest stack below the call). SSE runs on the `simd` model under the
//! caller's MXCSR, the x87 on the `x87` model under its control word, from
//! an empty stack. The refusals: `x87` (an unmasked x87 exception in the
//! control word, or one the model cannot make), `simd` (an unmasked SSE
//! exception raised), `floor_nan` (a NaN to `floor`: the CRT's error path
//! `_handle_qnan1`/`_except1`), `idiv` (a layer count of 0, or `INT_MIN`
//! over -1: the guest's #DE), `unread`, `unwritable`.

use std::collections::HashMap;

use engine::memory::{GuestMemory, MemFault};
use x87::abi::{self as fpu, arith_op, src_kind, unary_op, X87Abi};

/// Why the port refuses; the caller names it.
pub type Cause = &'static str;

/// The guest words the effect reads and writes.
pub mod address {
    /// The context's effect handle (the view's `+0x1b8`).
    pub const EFFECT: u32 = 0x0048_57b8;
    /// The tag table: a handle's low word picks the 16-byte entry whose
    /// `+8` is the definition.
    pub const TAGS: u32 = 0x004e_3b44;
    /// The time (double).
    pub const TIME: u32 = 0x0048_58a0;
    /// The context's view word and byte `+7`.
    pub const VIEW: u32 = 0x0048_5600;
    pub const VIEW_FLAG: u32 = 0x0048_5607;
    pub const ANGLE: u32 = 0x0048_562c;
    /// The window: top, left, bottom, right (signed words).
    pub const WINDOW: u32 = 0x0048_5648;
    pub const FOV: u32 = 0x0048_5640;
    pub const CAMERA: u32 = 0x0048_568c;
    pub const NEAR: u32 = 0x0048_57dc;
    pub const FAR: u32 = 0x0048_57e0;
    /// A pointer to the transform the step composes.
    pub const SOURCE: u32 = 0x0046_87d0;
    /// A pointer to the random state (its `+4` the seed).
    pub const RANDOM: u32 = 0x004e_7408;
    pub const INDEX: u32 = 0x004b_9a00;
    pub const RECORDS: u32 = 0x004b_9a04;
    pub const RECORD: u32 = 0x88;
    pub const LAYERS: u32 = 0x004b_9cac;
    pub const LAYER: u32 = 0x38;
    pub const KIND: u32 = 0x004b_9d8c;
    pub const SHARED_HALF: u32 = 0x004b_9d90;
    pub const SPREAD: u32 = 0x004b_9d94;
    pub const DENSITY: u32 = 0x004b_9d98;
    pub const FLAG: u32 = 0x004b_9d9c;
    /// Set once a layer's phase came out negative.
    pub const WARNED: u32 = 0x0055_e6c4;
}

/// A record's fields.
pub mod field {
    pub const READY: u32 = 0;
    pub const CAMERA: u32 = 4;
    pub const PHASE: u32 = 0x38;
    pub const ANGLE: u32 = 0x3c;
    pub const DEPTH: u32 = 0x40;
    pub const TIME: u32 = 0x44;
    /// Four (x, y) slots.
    pub const SLOTS: u32 = 0x48;
    pub const DIRECTION: u32 = 0x68;
    pub const TARGET: u32 = 0x70;
    pub const SPEED: u32 = 0x78;
    pub const TARGET_SPEED: u32 = 0x7c;
    pub const GUST_AT: u32 = 0x80;
    pub const GUST_PERIOD: u32 = 0x84;
}

/// Image constants, read at run time.
pub mod constant {
    pub const TWO_32: u32 = 0x0045_dbb8;
    pub const HALF: u32 = 0x0045_dbbc;
    pub const ONE: u32 = 0x0045_dbc0;
    pub const MINUS_HALF: u32 = 0x0045_dbc4;
    pub const MINUS_ONE: u32 = 0x0045_dbcc;
    pub const ZERO: u32 = 0x0045_dbd8;
    pub const SMALL: u32 = 0x0045_dbdc;
    pub const TWO: u32 = 0x0045_dc0c;
    pub const QUARTER: u32 = 0x0045_dc20;
    /// 1/65535: a random word as a value.
    pub const RANDOM_SCALE: u32 = 0x0045_dc50;
    pub const PI: u32 = 0x0045_dddc;
    /// The layers' weights 1..3 (layer 0's is 0).
    pub const WEIGHTS: [u32; 3] = [0x0045_e114, 0x0045_e110, 0x0045_e10c];
    /// The two `pow` exponents (doubles: 3 and 2).
    pub const CUBE: u32 = 0x0045_dee0;
    pub const SQUARE: u32 = 0x0045_ded0;
    /// `floor`'s control word (`0x173f`, round down).
    pub const FLOOR_CW: u32 = 0x0046_1f94;
}

use address::*;
use constant::*;

/// Make the effect on `mem` (**Code** `0x00027aa0`), with the caller's
/// MXCSR and x87 control word, from an empty x87 stack. Nothing is
/// written unless it all succeeds.
pub fn run(mem: &mut dyn GuestMemory, mxcsr: u32, x87_control: u16) -> Result<(), Cause> {
    if x87_control & 0x3f != 0x3f {
        return Err("x87");
    }
    let log = {
        let mut staged = Staged { base: &*mem, over: HashMap::new(), log: Vec::new() };
        body(&mut staged, mxcsr, x87_control)?;
        staged.log
    };
    for (at, data) in log {
        mem.write(at, &data).map_err(unwritable)?;
    }
    Ok(())
}

// ---- staged memory ----

/// Reads through the writes so far; writes kept in order.
struct Staged<'a> {
    base: &'a dyn GuestMemory,
    over: HashMap<u32, u8>,
    log: Vec<(u32, Vec<u8>)>,
}

impl GuestMemory for Staged<'_> {
    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        if self.base.read(addr, buf).is_err() {
            for (i, b) in buf.iter_mut().enumerate() {
                let a = addr.wrapping_add(i as u32);
                *b = match self.over.get(&a) {
                    Some(v) => *v,
                    None => self.base.u8(a)?,
                };
            }
            return Ok(());
        }
        for (i, b) in buf.iter_mut().enumerate() {
            if let Some(v) = self.over.get(&addr.wrapping_add(i as u32)) {
                *b = *v;
            }
        }
        Ok(())
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        for (i, b) in data.iter().enumerate() {
            self.over.insert(addr.wrapping_add(i as u32), *b);
        }
        self.log.push((addr, data.to_vec()));
        Ok(())
    }
}

fn unread(_: MemFault) -> Cause {
    "unread"
}

fn unwritable(_: MemFault) -> Cause {
    "unwritable"
}

fn rd(m: &dyn GuestMemory, a: u32) -> Result<u32, Cause> {
    m.u32(a).map_err(unread)
}

fn rd64(m: &dyn GuestMemory, a: u32) -> Result<u64, Cause> {
    Ok(u64::from(rd(m, a)?) | u64::from(rd(m, a.wrapping_add(4))?) << 32)
}

fn short(m: &dyn GuestMemory, a: u32) -> Result<i32, Cause> {
    Ok(i32::from(m.u16(a).map_err(unread)? as i16))
}

fn byte(m: &dyn GuestMemory, a: u32) -> Result<u8, Cause> {
    m.u8(a).map_err(unread)
}

fn put(m: &mut dyn GuestMemory, a: u32, v: u32) -> Result<(), Cause> {
    m.set_u32(a, v).map_err(unwritable)
}

fn put8(m: &mut dyn GuestMemory, a: u32, v: u8) -> Result<(), Cause> {
    m.set_u8(a, v).map_err(unwritable)
}

fn matrix(m: &dyn GuestMemory, a: u32) -> Result<[u32; 13], Cause> {
    let mut o = [0; 13];
    for (k, w) in o.iter_mut().enumerate() {
        *w = rd(m, a.wrapping_add(4 * k as u32))?;
    }
    Ok(o)
}

/// `rep movsd` of 13 words.
fn copy13(m: &mut dyn GuestMemory, to: u32, from: u32) -> Result<(), Cause> {
    for k in 0..13u32 {
        let w = rd(m, from.wrapping_add(4 * k))?;
        put(m, to.wrapping_add(4 * k), w)?;
    }
    Ok(())
}

/// The definition a handle names.
fn definition(m: &dyn GuestMemory, tags: u32, handle: u32) -> Result<u32, Cause> {
    rd(m, tags.wrapping_add((handle & 0xffff) << 4).wrapping_add(8))
}

fn lcg(s: u32) -> u32 {
    s.wrapping_mul(0x0019_660d).wrapping_add(0x3c6e_f35f)
}

/// The C remainder by 4 (`and 80000003h; jns; dec; or -4; inc`).
fn rem4(v: i32) -> i32 {
    v.wrapping_rem(4)
}

// ---- SSE ----

struct Sse(u32);

fn pack(v: [u32; 4]) -> u128 {
    v.iter().enumerate().fold(0u128, |a, (i, w)| a | u128::from(*w) << (32 * i))
}

fn unpack(v: u128) -> [u32; 4] {
    [v as u32, (v >> 32) as u32, (v >> 64) as u32, (v >> 96) as u32]
}

impl Sse {
    fn lane(r: Result<u128, simd::Xm>) -> Result<u32, Cause> {
        r.map(|v| v as u32).map_err(|_| "simd")
    }
    fn add(&mut self, a: u32, b: u32) -> Result<u32, Cause> {
        Self::lane(simd::addss(&mut self.0, a as u128, b))
    }
    fn sub(&mut self, a: u32, b: u32) -> Result<u32, Cause> {
        Self::lane(simd::subss(&mut self.0, a as u128, b))
    }
    fn mul(&mut self, a: u32, b: u32) -> Result<u32, Cause> {
        Self::lane(simd::mulss(&mut self.0, a as u128, b))
    }
    fn div(&mut self, a: u32, b: u32) -> Result<u32, Cause> {
        Self::lane(simd::divss(&mut self.0, a as u128, b))
    }
    fn int(&mut self, i: i32) -> Result<u32, Cause> {
        Self::lane(simd::cvtsi2ss(&mut self.0, 0, i as u32))
    }
    fn mulps(&mut self, a: [u32; 4], b: [u32; 4]) -> Result<[u32; 4], Cause> {
        simd::mulps(&mut self.0, pack(a), pack(b)).map(unpack).map_err(|_| "simd")
    }
    fn addps(&mut self, a: [u32; 4], b: [u32; 4]) -> Result<[u32; 4], Cause> {
        simd::addps(&mut self.0, pack(a), pack(b)).map(unpack).map_err(|_| "simd")
    }
    /// `comiss a, b; ja`.
    fn above(&mut self, a: u32, b: u32) -> Result<bool, Cause> {
        Ok(simd::comiss(&mut self.0, a, b).map_err(|_| "simd")? & 0x41 == 0)
    }
    /// `comiss a, b; jb`.
    fn below(&mut self, a: u32, b: u32) -> Result<bool, Cause> {
        Ok(simd::comiss(&mut self.0, a, b).map_err(|_| "simd")? & 0x01 != 0)
    }
    /// `ucomiss a, b; lahf; test ah, 44h; jnp`: ordered and equal.
    fn equal(&mut self, a: u32, b: u32) -> Result<bool, Cause> {
        Ok(simd::ucomiss(&mut self.0, a, b).map_err(|_| "simd")? & 0x44 == 0x40)
    }
    /// `lo` if `lo > v`, else `hi` if `v > hi`, else `v`.
    fn clamp(&mut self, v: u32, lo: u32, hi: u32) -> Result<u32, Cause> {
        Ok(if self.above(lo, v)? {
            lo
        } else if self.above(v, hi)? {
            hi
        } else {
            v
        })
    }
}

// ---- x87 ----

struct Fpu(X87Abi);

const C0: u32 = 0x100;
const C3: u32 = 0x4000;

impl Fpu {
    fn new(control: u16) -> Self {
        Fpu(X87Abi { control, ..Default::default() })
    }
    fn ok(code: u32) -> Result<u32, Cause> {
        if code & fpu::status::MASK == fpu::status::OK { Ok(code) } else { Err("x87") }
    }
    fn fld(&mut self, bits: u32) -> Result<(), Cause> {
        Self::ok(fpu::ld_f32(&mut self.0, bits)).map(drop)
    }
    fn fld_m64(&mut self, bits: u64) -> Result<(), Cause> {
        Self::ok(fpu::ld_f64(&mut self.0, bits)).map(drop)
    }
    fn fld_m80(&mut self, b: [u8; 10]) -> Result<(), Cause> {
        Self::ok(fpu::ld_f80(&mut self.0, b)).map(drop)
    }
    fn fild(&mut self, v: i32) -> Result<(), Cause> {
        Self::ok(fpu::ld_int(&mut self.0, i64::from(v))).map(drop)
    }
    fn fld_st(&mut self, i: u32) -> Result<(), Cause> {
        Self::ok(fpu::ld_st(&mut self.0, i)).map(drop)
    }
    /// `fld1` (0), `fldz` (1).
    fn fld_const(&mut self, which: u32) -> Result<(), Cause> {
        Self::ok(fpu::ld_const(&mut self.0, which)).map(drop)
    }
    /// `op st(0), m32`.
    fn arith_m(&mut self, op: u32, bits: u32) -> Result<(), Cause> {
        Self::ok(fpu::arith(&mut self.0, op, 0, src_kind::F32, u64::from(bits))).map(drop)
    }
    fn arith_m64(&mut self, op: u32, bits: u64) -> Result<(), Cause> {
        Self::ok(fpu::arith(&mut self.0, op, 0, src_kind::F64, bits)).map(drop)
    }
    /// `op st(0), st(i)`.
    fn arith_st(&mut self, op: u32, i: u32) -> Result<(), Cause> {
        Self::ok(fpu::arith(&mut self.0, op, 0, src_kind::ST, u64::from(i))).map(drop)
    }
    /// `op st(dst), st(src)`, popping after with `pop`.
    fn arith_reg(&mut self, op: u32, dst: u32, src: u32, pop: bool) -> Result<(), Cause> {
        let op = if pop { op | fpu::ARITH_POP } else { op };
        Self::ok(fpu::arith(&mut self.0, op, dst, src_kind::ST, u64::from(src))).map(drop)
    }
    fn unary(&mut self, op: u32) -> Result<(), Cause> {
        Self::ok(fpu::unary(&mut self.0, op)).map(drop)
    }
    fn fxch(&mut self, i: u32) -> Result<(), Cause> {
        Self::ok(fpu::xch(&mut self.0, i)).map(drop)
    }
    fn fstp_st(&mut self, i: u32) -> Result<(), Cause> {
        Self::ok(fpu::st_st(&mut self.0, i, true)).map(drop)
    }
    /// `fst(p) dword`.
    fn st32(&mut self, pop: bool) -> Result<u32, Cause> {
        let (code, store) = fpu::st_f32(&self.0, 0);
        Self::ok(code)?;
        fpu::store_commit(&mut self.0, store.status, pop);
        Ok(store.value)
    }
    /// `fst(p) qword`.
    fn st64(&mut self, pop: bool) -> Result<u64, Cause> {
        let (code, store) = fpu::st_f64(&self.0, 0);
        Self::ok(code)?;
        fpu::store_commit(&mut self.0, store.status, pop);
        Ok(store.value)
    }
    /// `fcomip st, st(i)`: ZF `0x40`, PF `0x04`, CF `0x01`.
    fn fcomip(&mut self, i: u32) -> Result<u32, Cause> {
        Ok(Self::ok(fpu::cmp(&mut self.0, 0, src_kind::ST, u64::from(i), 1, true))? & 0x45)
    }
    /// `fld m32; fcomip st, st(1); fstp st(0); ja`: `bits` above ST(0),
    /// which is popped too.
    fn above_pop(&mut self, bits: u32) -> Result<bool, Cause> {
        self.fld(bits)?;
        let f = self.fcomip(1)?;
        self.fstp_st(0)?;
        Ok(f & 0x41 == 0)
    }
    /// `fcom(p)(p) st(1)`: C3, C2, C0.
    fn fcom_st1(&mut self, pop: u32) -> Result<u32, Cause> {
        Ok(Self::ok(fpu::cmp(&mut self.0, 0, src_kind::ST, 1, pop, false))? & 0x4500)
    }
    fn fcom_m64(&mut self, bits: u64, pop: u32) -> Result<u32, Cause> {
        Ok(Self::ok(fpu::cmp(&mut self.0, 0, src_kind::F64, bits, pop, false))? & 0x4500)
    }
    fn ldcw(&mut self, cw: u16) -> Result<(), Cause> {
        Self::ok(fpu::ldcw(&mut self.0, cw)).map(drop)
    }
    /// `_ftol` `0x00322b0c`: ST(0) truncated to EDX:EAX, popped.
    fn ftol(&mut self) -> Result<u64, Cause> {
        let mut out = 0;
        Self::ok(fpu::ftol2(&mut self.0, &mut out))?;
        Ok(out)
    }
    /// `fild n; (jge) fadd 2^32; fmul [scale]`: a random word as a value.
    fn random(&mut self, m: &dyn GuestMemory, n: u32) -> Result<(), Cause> {
        self.fild(n as i32)?;
        if (n as i32) < 0 {
            self.arith_m(arith_op::ADD, rd(m, TWO_32)?)?;
        }
        self.arith_m(arith_op::MUL, rd(m, RANDOM_SCALE)?)
    }
}

/// `floor` (`0x00321099`) on the double `d`, pushed: `frndint` under the
/// control word `[0x00461f94]` (`0x00324a88`) and the caller's restored;
/// an infinity is its own floor. The caller's precision exception is
/// masked ([`run`] refuses otherwise), so an inexact result is kept. A NaN
/// goes to the CRT's error paths: refused.
fn floor(m: &dyn GuestMemory, x: &mut Fpu, d: u64) -> Result<(), Cause> {
    let saved = x.0.control;
    x.ldcw(rd(m, FLOOR_CW)? as u16)?;
    if (d >> 52) & 0x7ff == 0x7ff {
        if d & 0x000f_ffff_ffff_ffff != 0 {
            return Err("floor_nan");
        }
        x.ldcw(saved)?;
        return x.fld_m64(d);
    }
    x.fld_m64(d)?;
    x.unary(unary_op::RNDINT)?;
    let r = x.st64(true)?;
    x.ldcw(saved)?;
    x.fld_m64(r)
}

// ---- the CRT's pow 0x00372d48, as engine::function_eval has it ----

mod crt {
    pub const INDEFINITE: u32 = 0x0046_2738;
    pub const INFINITY_80: u32 = 0x0046_6e8c;
    pub const HALF_64: u32 = 0x0046_6e00;
    pub const DBL_MAX: u32 = 0x0041_6218;
    pub const DBL_MIN: u32 = 0x0041_6220;
    pub const DOWN: u32 = 0x0041_6228;
    pub const UP: u32 = 0x0041_6230;
    pub const OVER: u32 = 0x0041_6238;
    pub const UNDER: u32 = 0x0041_6240;
    pub const INFINITY_64: u32 = 0x0046_20b8;
    pub const INDEFINITE_64: u32 = 0x0046_20c0;
    pub const NEGATIVE_ZERO_64: u32 = 0x0046_20d8;
    pub const MAX_64: u32 = 0x0046_20c8;
    pub const ZERO_64: u32 = 0x0045_dbe0;
    pub const ONE_64: u32 = 0x0045_dcc0;
    pub const NO_REPORT: u32 = 0x0048_0cd4;
}

const SIGN_64: u64 = 1 << 63;
const QUIET_64: u64 = 1 << 51;

fn tbyte(m: &dyn GuestMemory, a: u32) -> Result<[u8; 10], Cause> {
    let mut b = [0u8; 10];
    m.read(a, &mut b).map_err(unread)?;
    Ok(b)
}

/// `_d_inttype` on a finite `y`: 0 not an integer, 1 odd, 2 even.
fn inttype(y: f64) -> u32 {
    if y.floor() != y {
        0
    } else if (y * 0.5).floor() == y * 0.5 {
        2
    } else {
        1
    }
}

/// `_powhlp` `0x003738a8`: either operand infinite, neither NaN; whether
/// it is an error.
fn powhlp(m: &dyn GuestMemory, x: u64, y: u64) -> Result<(u64, bool), Cause> {
    const POS_INF: u64 = 0x7ff0_0000_0000_0000;
    let inf = rd64(m, crt::INFINITY_64)?;
    let (xf, yf) = (f64::from_bits(x), f64::from_bits(y));
    let ax = if xf < 0.0 { -xf } else { xf };
    let one_64 = f64::from_bits(rd64(m, crt::ONE_64)?);
    let zero_64 = f64::from_bits(rd64(m, crt::ZERO_64)?);
    Ok(if y == POS_INF || y == POS_INF | SIGN_64 {
        let up = y == POS_INF;
        if ax > one_64 {
            (if up { inf } else { 0 }, false)
        } else if ax < one_64 {
            (if up { 0 } else { inf }, false)
        } else {
            (rd64(m, crt::INDEFINITE_64)?, true)
        }
    } else if x == POS_INF {
        if yf > zero_64 {
            (inf, false)
        } else if yf < zero_64 {
            (0, false)
        } else {
            (1.0f64.to_bits(), false)
        }
    } else {
        let odd = inttype(yf) == 1;
        if yf > zero_64 {
            (if odd { inf ^ SIGN_64 } else { inf }, false)
        } else if yf < zero_64 {
            (if odd { rd64(m, crt::NEGATIVE_ZERO_64)? } else { 0 }, false)
        } else {
            (1.0f64.to_bits(), false)
        }
    })
}

/// `0x00327fc9`: a double on; an infinity or NaN as the extended value
/// with the same bits.
fn load_double(x: &mut Fpu, d: u64) -> Result<(), Cause> {
    if (d >> 52) & 0x7ff == 0x7ff {
        let mut b = [0u8; 10];
        b[..8].copy_from_slice(&(d << 11).to_le_bytes());
        b[8..].copy_from_slice(&(((d >> 48) as u16) | 0x7fff).to_le_bytes());
        x.fld_m80(b)
    } else {
        x.fld_m64(d)
    }
}

/// `0x00372f31` on ST(0), popped: 0 not an integer, 1 odd, 2 even.
fn int_test(m: &dyn GuestMemory, x: &mut Fpu) -> Result<u32, Cause> {
    x.fld_st(0)?;
    x.unary(unary_op::RNDINT)?;
    if x.fcom_st1(1)? & C3 == 0 {
        x.fstp_st(0)?;
        return Ok(0);
    }
    x.arith_m64(arith_op::MUL, rd64(m, crt::HALF_64)?)?;
    x.fld_st(0)?;
    x.unary(unary_op::RNDINT)?;
    Ok(if x.fcom_st1(2)? & C3 == 0 { 1 } else { 2 })
}

/// `0x00327f84`: 2 to the ST(0).
fn two_to(x: &mut Fpu) -> Result<(), Cause> {
    x.fld_st(0)?;
    x.unary(unary_op::RNDINT)?;
    x.arith_reg(arith_op::SUBR, 1, 0, false)?;
    x.fxch(1)?;
    x.unary(unary_op::CHS)?;
    x.unary(unary_op::X2M1)?;
    x.fld_const(0)?;
    x.arith_reg(arith_op::ADD, 1, 0, true)?;
    x.unary(unary_op::SCALE)?;
    x.fstp_st(1)
}

enum Exit {
    Done,
    Range,
    Fail(u32),
}

/// `_handle_exc` `0x003246b5` for the masked exceptions.
fn handle_exc(m: &dyn GuestMemory, control: u16, kind: u32, v: u64) -> Result<u64, Cause> {
    let positive = f64::from_bits(v) > f64::from_bits(rd64(m, crt::ZERO_64)?);
    match kind {
        3 => {
            let inf = rd64(m, crt::INFINITY_64)?;
            let max = rd64(m, crt::MAX_64)?;
            Ok(match control & 0xc00 {
                0x000 => if positive { inf } else { inf ^ SIGN_64 },
                0x400 => if positive { max } else { inf ^ SIGN_64 },
                0x800 => if positive { inf } else { max ^ SIGN_64 },
                _ => if positive { max } else { max ^ SIGN_64 },
            })
        }
        4 => {
            let zero = rd64(m, crt::ZERO_64)?;
            if f64::from_bits(v) == f64::from_bits(zero) {
                return Ok(v);
            }
            let e = ((v >> 52) & 0x7ff) as i32;
            let mant = (v & !(0x7ff << 52)) | (0x3fe << 52);
            let expn = e - 0x3fe - 0x600;
            if expn < -1074 {
                let mut x = Fpu::new(control);
                x.fld_m64(mant)?;
                x.arith_m64(arith_op::MUL, zero)?;
                return x.st64(true);
            }
            let negative = f64::from_bits(mant) < f64::from_bits(zero);
            let mut bits = (mant & 0x000f_ffff_ffff_ffff) | 0x0010_0000_0000_0000;
            if expn < -1021 {
                bits >>= -1021 - expn;
            }
            Ok(if negative { bits ^ SIGN_64 } else { bits })
        }
        _ => Ok(v),
    }
}

/// `pow` (`0x00372d48`): ST(1) to the ST(0), both popped, the result
/// pushed, under `(cw & 0x300) | 0x7f` unless the caller's is `0x27f`.
fn pow(m: &dyn GuestMemory, x: &mut Fpu) -> Result<(), Cause> {
    let saved = x.0.control;
    if saved & 0x3f != 0x3f {
        return Err("x87");
    }
    x.fxch(1)?;
    let xd = x.st64(true)?;
    let pd = x.st64(false)?;
    if saved != 0x27f {
        x.ldcw((saved & 0x300) | 0x7f)?;
    }
    let exit = pow_body(m, x, xd, pd)?;
    let report = rd(m, crt::NO_REPORT)? == 0;
    match exit {
        Exit::Range if report => range(m, x, saved)?,
        Exit::Fail(kind) if report => fail(m, x, saved, kind)?,
        _ => {}
    }
    if saved != 0x27f {
        x.ldcw(saved)?;
    }
    Ok(())
}

fn pow_body(m: &dyn GuestMemory, x: &mut Fpu, xd: u64, pd: u64) -> Result<Exit, Cause> {
    let special = |d: u64| (d >> 52) & 0x7ff == 0x7ff;
    let nan = |d: u64| special(d) && d & 0x000f_ffff_ffff_ffff != 0;
    let signalling;
    if special(pd) {
        load_double(x, xd)?;
        if nan(pd) {
            if pd & QUIET_64 != 0 {
                x.arith_reg(arith_op::ADD, 1, 0, true)?;
                return Ok(Exit::Fail(1));
            }
            signalling = true;
        } else {
            signalling = false;
        }
    } else {
        load_double(x, xd)?;
        if !special(xd) {
            return finite(m, x, xd, pd);
        }
        signalling = false;
    }
    if nan(xd) {
        x.arith_reg(arith_op::ADD, 1, 0, true)?;
        return Ok(Exit::Fail(if xd & QUIET_64 != 0 { 1 } else { 7 }));
    }
    if signalling {
        x.arith_reg(arith_op::ADD, 1, 0, true)?;
        return Ok(Exit::Fail(7));
    }
    x.fstp_st(0)?;
    x.fstp_st(0)?;
    let (r, error) = powhlp(m, xd, pd)?;
    x.fld_m64(r)?;
    Ok(if error { Exit::Fail(1) } else { Exit::Done })
}

fn finite(m: &dyn GuestMemory, x: &mut Fpu, xd: u64, pd: u64) -> Result<Exit, Cause> {
    if (xd >> 52) & 0x7ff == 0 && xd & 0x000f_ffff_ffff_ffff == 0 {
        x.fstp_st(0)?;
        if pd & !SIGN_64 == 0 {
            x.fstp_st(0)?;
            x.fld_const(0)?;
            return Ok(Exit::Done);
        }
        let cl = int_test(m, x)?;
        let odd_negative = xd & SIGN_64 != 0 && cl == 1;
        if pd & SIGN_64 != 0 {
            x.fld_m80(tbyte(m, crt::INFINITY_80)?)?;
            if odd_negative {
                x.unary(unary_op::CHS)?;
            }
            return Ok(Exit::Fail(2));
        }
        x.fld_const(1)?;
        if odd_negative {
            x.unary(unary_op::CHS)?;
        }
        return Ok(Exit::Done);
    }
    let mut odd = false;
    if xd & SIGN_64 != 0 {
        x.fld_st(1)?;
        let cl = int_test(m, x)?;
        x.unary(unary_op::CHS)?;
        if cl == 0 {
            x.fstp_st(0)?;
            x.fstp_st(0)?;
            x.fld_m80(tbyte(m, crt::INDEFINITE)?)?;
            return Ok(Exit::Fail(1));
        }
        odd = cl == 1;
    }
    x.unary(unary_op::YL2X)?;
    two_to(x)?;
    if odd {
        x.unary(unary_op::CHS)?;
    }
    Ok(Exit::Range)
}

/// `0x0032806d`: a denormal, zero or infinite double result to the error
/// path, scaled.
fn range(m: &dyn GuestMemory, x: &mut Fpu, saved: u16) -> Result<(), Cause> {
    let d = x.st64(false)?;
    let e = (d >> 52) & 0x7ff;
    if e != 0 && e != 0x7ff {
        return Ok(());
    }
    let under = e == 0;
    x.fld_m64(rd64(m, if under { crt::UP } else { crt::DOWN })?)?;
    x.fxch(1)?;
    x.unary(unary_op::SCALE)?;
    x.fstp_st(1)?;
    x.fld_st(0)?;
    x.unary(unary_op::ABS)?;
    let c = x.fcom_m64(rd64(m, if under { crt::DBL_MIN } else { crt::DBL_MAX })?, 1)?;
    if under {
        if c & C0 != 0 {
            x.arith_m64(arith_op::MUL, rd64(m, crt::UNDER)?)?;
        }
        fail(m, x, saved, 4)
    } else {
        if c & (C0 | C3) == 0 {
            x.arith_m64(arith_op::MUL, rd64(m, crt::OVER)?)?;
        }
        fail(m, x, saved, 3)
    }
}

/// `0x00327f30`: the value through a double and `_87except`, back.
fn fail(m: &dyn GuestMemory, x: &mut Fpu, saved: u16, kind: u32) -> Result<(), Cause> {
    let v = x.st64(true)?;
    let v = handle_exc(m, saved, kind, v)?;
    x.fld_m64(v)
}

// ---- the transforms ----

/// `0x00141590` (ECX `i`, EAX out; **Code** `0x00141590..0x001416ba`): the
/// inverse of a 13-word transform (scale, three axes, position); zeros
/// when the scale is 0.
fn inverse(sse: &mut Sse, one: u32, i: &[u32; 13]) -> Result<[u32; 13], Cause> {
    if sse.equal(i[0], 0)? {
        return Ok([0; 13]);
    }
    let unit = sse.equal(i[0], one)?;
    let n = [sse.sub(0, i[10])?, sse.sub(0, i[11])?, sse.sub(0, i[12])?];
    let (s, t) = if unit {
        (one, n)
    } else {
        let k = sse.div(one, i[0])?;
        (k, [sse.mul(k, n[0])?, sse.mul(k, n[1])?, sse.mul(k, n[2])?])
    };
    let mut o = [0; 13];
    o[0] = s;
    // The axes transposed (integer moves: bits kept).
    for (to, from) in [(1, 1), (5, 5), (9, 9), (2, 4), (4, 2), (7, 3), (3, 7), (8, 6), (6, 8)] {
        o[to] = i[from];
    }
    let a = sse.mul(o[1], t[0])?;
    let b = sse.mul(o[7], t[2])?;
    let c = sse.add(b, a)?;
    let d = sse.mul(o[4], t[1])?;
    o[10] = sse.add(c, d)?;
    let a = sse.mul(o[2], t[0])?;
    let b = sse.mul(o[8], t[2])?;
    let c = sse.add(b, a)?;
    let d = sse.mul(t[1], o[5])?;
    o[11] = sse.add(c, d)?;
    let a = sse.mul(o[3], t[0])?;
    let b = sse.mul(o[9], t[2])?;
    let c = sse.add(a, b)?;
    let d = sse.mul(o[6], t[1])?;
    o[12] = sse.add(c, d)?;
    Ok(o)
}

/// `0x00142a60` (ECX `a`, EDX `b`, out; **Code** `0x00142a60..0x00142b6e`):
/// `a` applied to `b`, packed: an axis's components in lanes 0, 2 and 3
/// (`movss` then `movhps`), lane 1 zero.
fn compose(sse: &mut Sse, a: &[u32; 13], b: &[u32; 13]) -> Result<[u32; 13], Cause> {
    let splat = |v: u32| [v; 4];
    let row = |k: usize| [a[k], 0, a[k + 1], a[k + 2]];
    let (r0, r1, r2) = (row(1), row(4), row(7));
    let scale = splat(a[0]);
    let mut o = [0; 13];
    o[0] = sse.mul(b[0], a[0])?;
    let p0 = sse.mulps(splat(b[10]), r0)?;
    let p1 = sse.mulps(splat(b[11]), r1)?;
    let p2 = sse.mulps(splat(b[12]), r2)?;
    let s = sse.addps(p0, p1)?;
    let s = sse.addps(s, p2)?;
    let s = sse.mulps(s, scale)?;
    let s = sse.addps(s, row(10))?;
    (o[10], o[11], o[12]) = (s[0], s[2], s[3]);
    for k in [7, 4, 1] {
        let q2 = sse.mulps(splat(b[k + 2]), r2)?;
        let q1 = sse.mulps(splat(b[k + 1]), r1)?;
        let q0 = sse.mulps(splat(b[k]), r0)?;
        let s = sse.addps(q1, q2)?;
        let s = sse.addps(q0, s)?;
        (o[k], o[k + 1], o[k + 2]) = (s[0], s[2], s[3]);
    }
    Ok(o)
}

// ---- the wind 0x00028580 ----

/// `0x0002b400` (EAX `at`): the 2D vector at `at` normalised unless
/// shorter than `[SMALL]`; XMM0 out: its length, or `xmm0` as it came in.
fn normalise(m: &mut dyn GuestMemory, sse: &mut Sse, x: &mut Fpu, at: u32, xmm0: u32) -> Result<u32, Cause> {
    x.fld(rd(m, at.wrapping_add(4))?)?;
    x.fld(rd(m, at)?)?;
    x.fld_st(0)?;
    x.arith_st(arith_op::MUL, 1)?;
    x.fld_st(2)?;
    x.arith_st(arith_op::MUL, 3)?;
    x.arith_reg(arith_op::ADD, 1, 0, true)?;
    x.unary(unary_op::SQRT)?;
    x.fstp_st(2)?;
    x.fstp_st(0)?;
    let len = x.st32(false)?;
    x.unary(unary_op::ABS)?;
    if x.above_pop(rd(m, SMALL)?)? {
        return Ok(xmm0);
    }
    let inv = sse.div(rd(m, ONE)?, len)?;
    let nx = sse.mul(rd(m, at)?, inv)?;
    let ny = sse.mul(inv, rd(m, at.wrapping_add(4))?)?;
    put(m, at, nx)?;
    put(m, at.wrapping_add(4), ny)?;
    Ok(len)
}

/// `0x00028580(time, out)` (EAX the effect handle; **Code**
/// `0x00028580..0x000288d4`): the record of the index as it is now eases
/// its direction and speed toward their targets and, when its gust period
/// has passed, draws new ones; the wind (x, y, 0) is the direction times
/// the speed times the time since its last call.
fn wind(m: &mut dyn GuestMemory, sse: &mut Sse, x: &mut Fpu, effect: u32, time: u32) -> Result<[u32; 3], Cause> {
    let tags = rd(m, TAGS)?;
    let r = RECORDS.wrapping_add(rd(m, INDEX)?.wrapping_mul(RECORD));
    let f = |off: u32| r.wrapping_add(off);
    let def = definition(m, tags, effect)?;
    let d = |off: u32| def.wrapping_add(off);
    let delta = sse.sub(time, rd(m, f(field::TIME))?)?;
    put(m, f(field::TIME), time)?;
    if sse.above(rd(m, d(0x30))?, 0)? {
        let one = rd(m, ONE)?;
        let ease = sse.sub(one, rd(m, d(0x3c))?)?;
        let x1 = sse.mul(rd(m, f(field::DIRECTION))?, ease)?;
        let y1 = sse.mul(rd(m, f(field::DIRECTION + 4))?, ease)?;
        put(m, f(field::DIRECTION), x1)?;
        put(m, f(field::DIRECTION + 4), y1)?;
        let k = rd(m, d(0x3c))?;
        let v = sse.mul(rd(m, f(field::TARGET))?, k)?;
        let nx = sse.add(v, x1)?;
        put(m, f(field::DIRECTION), nx)?;
        x.fld(rd(m, f(field::DIRECTION))?)?;
        let v = sse.mul(rd(m, f(field::TARGET + 4))?, k)?;
        let ny = sse.add(v, y1)?;
        x.fld(ny)?;
        put(m, f(field::DIRECTION + 4), ny)?;
        x.arith_m(arith_op::MUL, ny)?;
        x.fld_st(1)?;
        x.arith_st(arith_op::MUL, 2)?;
        x.arith_reg(arith_op::ADD, 1, 0, true)?;
        x.unary(unary_op::SQRT)?;
        x.fstp_st(1)?;
        let len = x.st32(false)?;
        x.unary(unary_op::ABS)?;
        let len = if x.above_pop(rd(m, SMALL)?)? {
            0
        } else {
            let inv = sse.div(one, len)?;
            let v = sse.mul(rd(m, f(field::DIRECTION))?, inv)?;
            put(m, f(field::DIRECTION), v)?;
            let v = sse.mul(rd(m, f(field::DIRECTION + 4))?, inv)?;
            put(m, f(field::DIRECTION + 4), v)?;
            len
        };
        if sse.equal(len, 0)? {
            put(m, f(field::DIRECTION), one)?;
            put(m, f(field::DIRECTION + 4), 0)?;
        }
        let k = rd(m, d(0x3c))?;
        let since = sse.sub(time, rd(m, f(field::GUST_AT))?)?;
        let early = sse.below(since, rd(m, f(field::GUST_PERIOD))?)?;
        let a = sse.sub(one, k)?;
        let a = sse.mul(a, rd(m, f(field::SPEED))?)?;
        let b = sse.mul(k, rd(m, f(field::TARGET_SPEED))?)?;
        put(m, f(field::SPEED), sse.add(a, b)?)?;
        if !early {
            gust(m, sse, x, r, def, time, one)?;
        }
    }
    let xm = sse.mul(rd(m, f(field::DIRECTION))?, rd(m, f(field::SPEED))?)?;
    let xm = sse.mul(xm, delta)?;
    let ym = sse.mul(rd(m, f(field::DIRECTION + 4))?, rd(m, f(field::SPEED))?)?;
    let ym = sse.mul(ym, delta)?;
    Ok([xm, ym, 0])
}

/// `0x00028701..0x00028892`: a new target direction (a random turn of
/// the current by `pow(random, 1 - [def+0x40])`, either way), speed
/// (`+0x2c..+0x30`) and period (`+0x34..+0x38`).
fn gust(m: &mut dyn GuestMemory, sse: &mut Sse, x: &mut Fpu, r: u32, def: u32, time: u32, one: u32) -> Result<(), Cause> {
    let f = |off: u32| r.wrapping_add(off);
    let d = |off: u32| def.wrapping_add(off);
    let p = rd(m, RANDOM)?.wrapping_add(4);
    let s = lcg(rd(m, p)?);
    put(m, p, s)?;
    x.random(m, s >> 16)?;
    x.fld(one)?;
    x.arith_m(arith_op::SUB, rd(m, d(0x40))?)?;
    pow(m, x)?;
    let s = lcg(rd(m, p)?);
    let sign = if s & 0x8000_0000 != 0 { rd(m, MINUS_ONE)? } else { one };
    let pw = x.st32(true)?;
    put(m, p, s)?;
    let neg_y = sse.sub(0, rd(m, f(field::DIRECTION + 4))?)?;
    let turn = sse.mul(sign, pw)?;
    let a = sse.mul(turn, neg_y)?;
    let b = sse.mul(rd(m, f(field::DIRECTION))?, turn)?;
    let keep = sse.sub(one, pw)?;
    let tx = sse.mul(rd(m, f(field::DIRECTION))?, keep)?;
    let tx = sse.add(tx, a)?;
    put(m, f(field::TARGET), tx)?;
    let ty = sse.mul(rd(m, f(field::DIRECTION + 4))?, keep)?;
    let ty = sse.add(ty, b)?;
    put(m, f(field::TARGET + 4), ty)?;
    let len = normalise(m, sse, x, f(field::TARGET), keep)?;
    if sse.equal(len, 0)? {
        put(m, f(field::TARGET), one)?;
        put(m, f(field::TARGET + 4), 0)?;
    }
    for (lo, hi, to, at) in [(0x2c, 0x30, field::TARGET_SPEED, Some(field::GUST_AT)), (0x34, 0x38, field::GUST_PERIOD, None)] {
        let s = lcg(rd(m, p)?);
        x.fld(rd(m, d(hi))?)?;
        x.fld(rd(m, d(lo))?)?;
        put(m, p, s)?;
        x.random(m, s >> 16)?;
        if let Some(at) = at {
            put(m, f(at), time)?;
        }
        x.fxch(2)?;
        x.arith_st(arith_op::SUB, 1)?;
        x.arith_reg(arith_op::MUL, 2, 0, true)?;
        x.fxch(1)?;
        x.arith_st(arith_op::ADD, 1)?;
        let v = x.st32(true)?;
        put(m, f(to), v)?;
        x.fstp_st(0)?;
    }
    Ok(())
}

// ---- the body ----

/// Two LCG steps, both written; the second's high word as a value in
/// SSE (`cvtsi2ss` of the word read back, times the scale) and x87.
fn slot_random(m: &mut dyn GuestMemory, sse: &mut Sse, x: &mut Fpu) -> Result<(u32, u32), Cause> {
    let p = rd(m, RANDOM)?.wrapping_add(4);
    let s = lcg(rd(m, p)?);
    put(m, p, s)?;
    let s = lcg(s);
    put(m, p, s)?;
    let word = i32::from(m.u16(p.wrapping_add(2)).map_err(unread)?);
    let y = sse.int(word)?;
    let y = sse.mul(y, rd(m, RANDOM_SCALE)?)?;
    x.random(m, s >> 16)?;
    Ok((y, x.st32(true)?))
}

fn body(m: &mut dyn GuestMemory, mxcsr: u32, cw: u16) -> Result<(), Cause> {
    let effect = rd(m, EFFECT)?;
    if effect == u32::MAX || effect == 0 {
        return Ok(());
    }
    let mut sse = Sse(mxcsr);
    let mut x = Fpu::new(cw);
    let tags = rd(m, TAGS)?;
    x.fld_m64(rd64(m, TIME)?)?;
    let time = x.st32(true)?;
    let old = rd(m, INDEX)?;
    let def = definition(m, tags, effect)?;
    let d = |off: u32| def.wrapping_add(off);
    let layers_def = definition(m, tags, rd(m, d(0x18))?)?;
    let r = RECORDS.wrapping_add(old.wrapping_mul(RECORD));
    let f = |off: u32| r.wrapping_add(off);
    let view = short(m, VIEW)?;
    let flagged = byte(m, VIEW_FLAG)? != 0;
    put(m, INDEX, view as u32)?;
    if flagged {
        put(m, INDEX, 4)?;
    }

    // 0x00027b2c: the shared words.
    let (one, half) = (rd(m, ONE)?, rd(m, HALF)?);
    x.fld(rd(m, ANGLE)?)?;
    x.arith_m(arith_op::MUL, rd(m, PI)?)?;
    put(m, SHARED_HALF, half)?;
    x.unary(unary_op::COS)?;
    put(m, KIND, 0x13)?;
    x.arith_m(arith_op::ADD, one)?;
    x.arith_m(arith_op::MUL, half)?;
    let c = x.st32(true)?;
    let rest = sse.sub(one, c)?;
    let a = sse.mul(rd(m, d(0x20))?, rest)?;
    let b = sse.mul(rd(m, d(0x1c))?, c)?;
    put(m, SPREAD, sse.add(a, b)?)?;
    let a = sse.mul(rd(m, d(0x24))?, rest)?;
    put(m, DENSITY, sse.add(a, c)?)?;
    let flag = byte(m, def)? & 1;
    put8(m, FLAG, flag)?;
    let mut source = matrix(m, rd(m, SOURCE)?)?;

    // 0x00027bd5: a fresh record.
    if byte(m, f(field::READY))? == 0 {
        m.write(r, &[0; RECORD as usize]).map_err(unwritable)?;
        for k in 0..4 {
            let (y, v) = slot_random(m, &mut sse, &mut x)?;
            put(m, f(field::SLOTS + 8 * k + 4), y)?;
            put(m, f(field::SLOTS + 8 * k), v)?;
        }
        copy13(m, f(field::CAMERA), CAMERA)?;
        put8(m, f(field::READY), 1)?;
    }

    // 0x00027d44: the wind, over the source's position (the frame's
    // overlap of the two locals).
    let w = wind(m, &mut sse, &mut x, effect, time)?;
    for k in 0..3 {
        source[10 + k] = sse.add(rd(m, d(0x44 + 4 * k as u32))?, w[k])?;
    }
    let inv = inverse(&mut sse, one, &matrix(m, f(field::CAMERA))?)?;
    let step = compose(&mut sse, &source, &inv)?;
    let mat = compose(&mut sse, &matrix(m, CAMERA)?, &step)?;
    let (far, near) = (rd(m, FAR)?, rd(m, NEAR)?);
    let deep = sse.above(far, near)?;
    copy13(m, f(field::CAMERA), CAMERA)?;
    if !deep {
        return Ok(());
    }

    // 0x00027de2: the layers' angle and scale.
    let small = rd(m, SMALL)?;
    let span = sse.sub(far, near)?;
    let q = sse.mul(span, rd(m, QUARTER)?)?;
    let span = sse.sub(far, near)?;
    let span = if sse.above(small, span)? { small } else { sse.sub(far, near)? };
    let far_small = sse.above(small, far)?;
    let inv_span = sse.div(one, span)?;
    let h = sse.int(short(m, WINDOW + 4)?)?;
    let h = sse.sub(h, sse.int(short(m, WINDOW)?)?)?;
    let wd = sse.int(short(m, WINDOW + 6)?)?;
    let wd = sse.sub(wd, sse.int(short(m, WINDOW + 2)?)?)?;
    let aspect = sse.div(h, wd)?;
    let far_ = if far_small { small } else { far };
    let q_small = sse.above(small, q)?;
    x.fld(rd(m, d(0x10))?)?;
    x.arith_m(arith_op::MUL, half)?;
    x.fld(rd(m, FOV)?)?;
    x.arith_m(arith_op::MUL, half)?;
    x.unary(unary_op::TAN)?;
    x.fstp_st(0)?;
    x.fxch(1)?;
    x.fxch(1)?;
    x.arith_m(arith_op::MUL, far_)?;
    x.arith_m(arith_op::MUL, aspect)?;
    x.arith_reg(arith_op::DIV, 1, 0, true)?;
    let k28 = x.st32(true)?;
    let zero_c = rd(m, ZERO)?;
    for (a, b, c) in [(4, 7, 1), (5, 8, 2)] {
        x.fld(mat[a])?;
        x.arith_m(arith_op::ADD, mat[b])?;
        x.arith_m(arith_op::MUL, zero_c)?;
        x.arith_m(arith_op::ADD, mat[c])?;
    }
    x.fxch(1)?;
    x.unary(unary_op::PATAN)?;
    x.arith_m(arith_op::MUL, rd(m, d(4))?)?;
    x.unary(unary_op::CHS)?;
    x.arith_m(arith_op::SUBR, rd(m, f(field::ANGLE))?)?;
    let angle = x.st32(false)?;
    put(m, f(field::ANGLE), angle)?;
    x.fld_st(0)?;
    x.unary(unary_op::COS)?;
    let cos = x.st32(true)?;
    x.unary(unary_op::SIN)?;
    let sin = x.st32(true)?;

    // 0x00027eec: the depth and its wrap.
    let q_ = if q_small { small } else { q };
    let t = sse.div(rd(m, d(0xc))?, q_)?;
    let t = sse.mul(t, mat[12])?;
    let depth = sse.sub(rd(m, f(field::DEPTH))?, t)?;
    x.fld(depth)?;
    let arg = x.st64(true)?;
    put(m, f(field::DEPTH), depth)?;
    floor(m, &mut x, arg)?;
    let whole = x.st32(true)?;
    x.fld(rd(m, f(field::DEPTH))?)?;
    x.arith_m(arith_op::SUB, whole)?;
    let v = x.st32(true)?;
    put(m, f(field::DEPTH), v)?;
    x.fld(whole)?;
    let n = x.ftol()? as u32;
    let phase = (rd(m, f(field::PHASE))? as i32).wrapping_sub(n as i32);
    put(m, f(field::PHASE), phase as u32)?;
    if phase < 0 {
        put(m, f(field::PHASE), phase.rem_euclid(4) as u32)?;
    }
    let phase = rd(m, f(field::PHASE))? as i32;
    if phase >= 4 {
        put(m, f(field::PHASE), (phase % 4) as u32)?;
    }
    let slot_at = |k: i32| r.wrapping_add(field::SLOTS).wrapping_add((k as u32).wrapping_mul(8));
    if sse.above(0, whole)? {
        let k = rem4((rd(m, f(field::PHASE))? as i32).wrapping_add(3));
        let (y, v) = slot_random(m, &mut sse, &mut x)?;
        put(m, slot_at(k).wrapping_add(4), y)?;
        put(m, slot_at(k), v)?;
    } else if sse.above(whole, 0)? {
        let p = rd(m, RANDOM)?.wrapping_add(4);
        let s1 = lcg(rd(m, p)?);
        let s2 = lcg(s1);
        put(m, p, s1)?;
        put(m, p, s2)?;
        let word = i32::from(m.u16(p.wrapping_add(2)).map_err(unread)?);
        let k = rd(m, f(field::PHASE))? as i32;
        let y = sse.int(word)?;
        let y = sse.mul(y, rd(m, RANDOM_SCALE)?)?;
        x.random(m, s2 >> 16)?;
        put(m, slot_at(k).wrapping_add(4), y)?;
        let v = x.st32(true)?;
        put(m, slot_at(k), v)?;
    }
    if sse.above(0, rd(m, f(field::DEPTH))?)? {
        return Ok(());
    }
    let depth = rd(m, f(field::DEPTH))?;
    if !sse.below(depth, one)? {
        return Ok(());
    }

    // 0x0002806c: each layer's phase.
    let weights = [0, rd(m, WEIGHTS[0])?, rd(m, WEIGHTS[1])?, rd(m, WEIGHTS[2])?];
    let count_at = layers_def.wrapping_add(0x44);
    for i in 0..4u32 {
        let layer = LAYERS + i * LAYER;
        let b28 = rd(m, d(0x28))?;
        if sse.above(b28, 0)? {
            x.fild(rd(m, count_at)? as i32)?;
            x.arith_m(arith_op::MUL, weights[i as usize])?;
            x.fld(rd(m, f(field::TIME))?)?;
            x.arith_m(arith_op::DIV, rd(m, d(0x28))?)?;
            x.arith_reg(arith_op::ADD, 1, 0, true)?;
            let s32 = x.st32(false)?;
            let s64 = x.st64(false)?;
            let arg = x.st64(true)?;
            floor(m, &mut x, arg)?;
            let n = x.ftol()? as i32;
            x.fld_m64(s64)?;
            let arg = x.st64(true)?;
            let count = rd(m, count_at)? as i32;
            if count == 0 || (n == i32::MIN && count == -1) {
                return Err("idiv");
            }
            put(m, layer, (n % count) as u32)?;
            floor(m, &mut x, arg)?;
            x.arith_m(arith_op::SUBR, s32)?;
            let g = if x.above_pop(zero_c)? {
                0
            } else {
                let again = |m: &dyn GuestMemory, x: &mut Fpu| -> Result<(), Cause> {
                    x.fld_m64(s64)?;
                    let arg = x.st64(true)?;
                    floor(m, x, arg)?;
                    x.arith_m(arith_op::SUBR, s32)
                };
                again(&*m, &mut x)?;
                x.fld(one)?;
                x.fxch(1)?;
                let fl = x.fcomip(1)?;
                x.fstp_st(0)?;
                if fl & 0x41 == 0 {
                    one
                } else {
                    again(&*m, &mut x)?;
                    x.st32(true)?
                }
            };
            let before = rd(m, layer)? as i32;
            put(m, layer + 4, g)?;
            if before < 0 {
                if byte(m, WARNED)? == 0 {
                    put8(m, WARNED, 1)?;
                }
                put(m, layer, 0)?;
            }
        } else {
            let count = rd(m, count_at)? as i32;
            if count == 0 {
                return Err("idiv");
            }
            put(m, layer + 4, 0)?;
            put(m, layer, (i as i32 % count) as u32)?;
        }
    }

    // 0x000281d0: each layer's weights and transform.
    let (two, minus_half) = (rd(m, TWO)?, rd(m, MINUS_HALF)?);
    for j in 0..4i32 {
        let k = rem4((rd(m, f(field::PHASE))? as i32).wrapping_add(j));
        let near = rd(m, NEAR)?;
        let v = sse.int(j)?;
        let v = sse.add(v, rd(m, f(field::DEPTH))?)?;
        let v = sse.mul(v, q)?;
        let dist = sse.add(v, near)?;
        let t = sse.sub(dist, near)?;
        let t = sse.mul(t, inv_span)?;
        let e = sse.mul(t, two)?;
        let e = sse.sub(e, one)?;
        let a = if sse.below(e, 0)? { sse.sub(0, e)? } else { e };
        x.fld(a)?;
        x.fld_m64(rd64(m, CUBE)?)?;
        pow(m, &mut x)?;
        x.st32(false)?;
        x.arith_m(arith_op::SUBR, one)?;
        x.fld_m64(rd64(m, SQUARE)?)?;
        pow(m, &mut x)?;
        let far = rd(m, FAR)?;
        let small = rd(m, SMALL)?;
        let far_small = sse.above(small, far)?;
        x.arith_m(arith_op::MUL, rd(m, DENSITY)?)?;
        let w = x.st32(true)?;
        let far_ = if far_small { small } else { far };
        let s6 = sse.div(rd(m, d(0x10))?, far_)?;
        let neg = sse.sub(0, dist)?;
        let s6 = sse.mul(s6, dist)?;
        let (x3, x4, x5) = if sse.equal(mat[0], one)? {
            (0, 0, neg)
        } else {
            let z = sse.mul(mat[0], 0)?;
            (z, z, sse.mul(mat[0], neg)?)
        };
        let p = sse.mul(mat[4], x4)?;
        let px = sse.mul(mat[7], x5)?;
        let px = sse.add(px, p)?;
        let p = sse.mul(mat[1], x3)?;
        let px = sse.add(px, p)?;
        let px = sse.add(px, mat[10])?;
        let py = sse.mul(mat[8], x5)?;
        let p = sse.mul(mat[5], x4)?;
        let py = sse.add(py, p)?;
        let p = sse.mul(mat[2], x3)?;
        let py = sse.add(py, p)?;
        let py = sse.add(py, mat[11])?;
        let spread = rd(m, SPREAD)?;
        let slot = slot_at(k);
        let u = sse.mul(py, sin)?;
        let u = sse.mul(u, spread)?;
        let pc = sse.mul(py, cos)?;
        let xc = sse.mul(px, cos)?;
        let xs = sse.mul(px, sin)?;
        let u = sse.add(u, xc)?;
        let u = sse.mul(u, rd(m, d(8))?)?;
        let u = sse.mul(u, k28)?;
        let u = sse.mul(u, half)?;
        let v = sse.sub(rd(m, slot)?, u)?;
        put(m, slot, v)?;
        let u = sse.mul(pc, spread)?;
        let u = sse.sub(u, xs)?;
        let u = sse.mul(u, rd(m, d(8))?)?;
        let u = sse.mul(u, k28)?;
        let u = sse.mul(u, half)?;
        let v = sse.sub(rd(m, slot.wrapping_add(4))?, u)?;
        put(m, slot.wrapping_add(4), v)?;

        let lay = LAYERS.wrapping_add((k as u32).wrapping_mul(LAYER));
        let l = |off: u32| lay.wrapping_add(off);
        if !sse.equal(rd(m, d(0x28))?, 0)? {
            let tc = sse.clamp(t, 0, one)?;
            let g = rd(m, l(4))?;
            let v = sse.mul(g, g)?;
            let v = sse.mul(v, w)?;
            let far_w = sse.clamp(v, 0, one)?;
            let rest = sse.sub(one, g)?;
            let v = sse.mul(g, two)?;
            let v = sse.mul(v, rest)?;
            let v = sse.mul(v, w)?;
            let mid_w = sse.clamp(v, 0, one)?;
            let v = sse.mul(rest, rest)?;
            let v = sse.mul(v, w)?;
            let near_w = sse.clamp(v, 0, one)?;
            put(m, l(8), near_w)?;
            put(m, l(0x10), far_w)?;
            put(m, l(0xc), mid_w)?;
            put(m, l(0x14), tc)?;
        } else {
            let tc = sse.clamp(t, 0, one)?;
            let wc = sse.clamp(w, 0, one)?;
            put(m, l(8), 0)?;
            put(m, l(0xc), wc)?;
            put(m, l(0x10), 0)?;
            put(m, l(0x14), tc)?;
        }
        let sx = rd(m, slot)?;
        let a0 = sse.mul(s6, cos)?;
        let a1 = sse.mul(s6, sin)?;
        let v = sse.mul(a0, half)?;
        put(m, l(0x18), v)?;
        let v2 = sse.mul(a1, minus_half)?;
        put(m, l(0x20), 0)?;
        put(m, l(0x24), sx)?;
        let v = sse.mul(a1, aspect)?;
        let v = sse.mul(v, half)?;
        put(m, l(0x1c), v)?;
        let sy = rd(m, slot.wrapping_add(4))?;
        let v4 = sse.mul(a0, aspect)?;
        let v4 = sse.mul(v4, half)?;
        put(m, l(0x28), v2)?;
        put(m, l(0x2c), v4)?;
        put(m, l(0x30), 0)?;
        put(m, l(0x34), sy)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::memory::FlatMemory;

    const MXCSR: u32 = 0x1f80;
    const CW: u16 = 0x027f;
    const TABLE: u32 = 0x0055_0000;
    const DEF: u32 = 0x0055_1000;
    const LAYERS_DEF: u32 = 0x0055_2000;
    const RNG: u32 = 0x0055_3000;
    const SRC: u32 = 0x0055_4000;
    const R0: u32 = RECORDS;

    fn f(v: f32) -> u32 {
        v.to_bits()
    }

    fn rf(m: &FlatMemory, a: u32) -> f32 {
        m.f32(a).unwrap()
    }

    fn set(m: &mut FlatMemory, a: u32, v: f32) {
        m.set_u32(a, v.to_bits()).unwrap();
    }

    fn set_matrix(m: &mut FlatMemory, a: u32, v: [f32; 13]) {
        for (k, w) in v.iter().enumerate() {
            set(m, a + 4 * k as u32, *w);
        }
    }

    const IDENTITY: [f32; 13] = [1.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];

    /// The image's constants, a definition with no wind and no layer
    /// scroll, a ready record, far not above near.
    fn world() -> FlatMemory {
        let mut m = FlatMemory::new(0x0040_0000, 0x0016_0000);
        for (a, v) in [
            (TWO_32, 4294967296.0f32),
            (HALF, 0.5),
            (ONE, 1.0),
            (MINUS_HALF, -0.5),
            (MINUS_ONE, -1.0),
            (ZERO, 0.0),
            (SMALL, f32::from_bits(0x38d1_b717)),
            (TWO, 2.0),
            (QUARTER, 0.25),
            (RANDOM_SCALE, f32::from_bits(0x3780_0080)),
            (PI, std::f32::consts::PI),
            (WEIGHTS[0], 0.7135),
            (WEIGHTS[1], 0.3422),
            (WEIGHTS[2], 0.5798),
        ] {
            set(&mut m, a, v);
        }
        m.set_u32(FLOOR_CW, 0x173f).unwrap();
        for (a, v) in [(CUBE, 3.0f64), (SQUARE, 2.0), (crt::ONE_64, 1.0), (crt::HALF_64, 0.5)] {
            m.write(a, &v.to_bits().to_le_bytes()).unwrap();
        }
        m.write(TIME, &10.0f64.to_bits().to_le_bytes()).unwrap();
        m.set_u32(EFFECT, 1).unwrap();
        m.set_u32(TAGS, TABLE).unwrap();
        m.set_u32(TABLE + 0x10 + 8, DEF).unwrap();
        m.set_u32(TABLE + 0x20 + 8, LAYERS_DEF).unwrap();
        m.set_u32(DEF + 0x18, 2).unwrap();
        m.set_u32(LAYERS_DEF + 0x44, 4).unwrap();
        m.set_u32(RANDOM, RNG).unwrap();
        m.set_u32(RNG + 4, 0x1234_5678).unwrap();
        m.set_u32(SOURCE, SRC).unwrap();
        set_matrix(&mut m, SRC, IDENTITY);
        set_matrix(&mut m, CAMERA, IDENTITY);
        set_matrix(&mut m, R0 + field::CAMERA, IDENTITY);
        m.set_u8(R0, 1).unwrap();
        set(&mut m, R0 + field::DIRECTION, 1.0);
        set(&mut m, DEF + 0x1c, 3.0);
        set(&mut m, DEF + 0x20, 5.0);
        set(&mut m, DEF + 0x24, 7.0);
        m.set_u8(DEF, 0x81).unwrap();
        for (a, v) in [(WINDOW, 0i16), (WINDOW + 2, 0), (WINDOW + 4, 480), (WINDOW + 6, 640)] {
            m.write(a, &v.to_le_bytes()).unwrap();
        }
        set(&mut m, FOV, 1.0);
        m
    }

    /// Far above near: the layers run. Depth 0.5, a quarter of the span 1.
    fn deep(m: &mut FlatMemory) {
        set(m, NEAR, 0.0);
        set(m, FAR, 4.0);
        set(m, R0 + field::DEPTH, 0.5);
    }

    fn all(m: &FlatMemory) -> Vec<u8> {
        m.bytes(0x0040_0000, 0x0016_0000).unwrap()
    }

    #[test]
    fn no_effect_writes_nothing() {
        for handle in [0, u32::MAX] {
            let mut m = world();
            m.set_u32(EFFECT, handle).unwrap();
            let before = all(&m);
            run(&mut m, MXCSR, CW).unwrap();
            assert_eq!(all(&m), before);
        }
    }

    #[test]
    fn an_unmasked_control_word_is_refused_and_writes_nothing() {
        let mut m = world();
        let before = all(&m);
        assert_eq!(run(&mut m, MXCSR, 0x027e), Err("x87"));
        assert_eq!(all(&m), before);
    }

    #[test]
    fn the_shared_words_follow_the_angle() {
        let mut m = world();
        run(&mut m, MXCSR, CW).unwrap();
        // cos(0) = 1: c = 1, so the spread is +0x1c and the density 1.
        assert_eq!(m.u32(KIND).unwrap(), 0x13);
        assert_eq!(rf(&m, SHARED_HALF), 0.5);
        assert_eq!(rf(&m, SPREAD), 3.0);
        assert_eq!(rf(&m, DENSITY), 1.0);
        assert_eq!(m.u8(FLAG).unwrap(), 1);
        assert_eq!(m.u32(INDEX).unwrap(), 0);
        // The record keeps the camera; far not above near ends it.
        assert_eq!(m.u32(R0 + field::CAMERA).unwrap(), f(1.0));
        assert_eq!(rf(&m, R0 + field::TIME), 10.0);
        assert_eq!(m.u32(LAYERS + 4).unwrap(), 0);
    }

    #[test]
    fn the_views_byte_picks_index_four() {
        let mut m = world();
        m.set_u8(VIEW_FLAG, 1).unwrap();
        m.write(VIEW, &2u16.to_le_bytes()).unwrap();
        run(&mut m, MXCSR, CW).unwrap();
        assert_eq!(m.u32(INDEX).unwrap(), 4);
        // The wind runs on the record the new index picks; the old one
        // takes the camera.
        assert_eq!(rf(&m, R0 + 4 * RECORD + field::TIME), 10.0);
        assert_eq!(m.u32(R0 + field::TIME).unwrap(), 0);
    }

    #[test]
    fn a_fresh_record_is_zeroed_and_takes_four_random_slots() {
        let mut m = world();
        m.set_u8(R0, 0).unwrap();
        set(&mut m, R0 + field::DIRECTION, 9.0);
        set_matrix(&mut m, CAMERA, [2.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 2.0, 3.0]);
        run(&mut m, MXCSR, CW).unwrap();
        assert_eq!(m.u8(R0).unwrap(), 1);
        assert_eq!(rf(&m, R0 + field::DIRECTION), 0.0);
        let mut s = 0x1234_5678u32;
        for k in 0..4 {
            s = lcg(lcg(s));
            let v = (s >> 16) as f32 * f32::from_bits(0x3780_0080);
            assert_eq!(rf(&m, R0 + field::SLOTS + 8 * k), v);
            assert_eq!(rf(&m, R0 + field::SLOTS + 8 * k + 4), v);
        }
        assert_eq!(m.u32(RNG + 4).unwrap(), s);
        assert_eq!(rf(&m, R0 + field::CAMERA), 2.0);
        assert_eq!(rf(&m, R0 + field::CAMERA + 0x30), 3.0);
    }

    #[test]
    fn the_wind_eases_and_waits_for_its_gust() {
        let mut m = world();
        set(&mut m, DEF + 0x30, 1.0);
        set(&mut m, DEF + 0x3c, 0.5);
        set(&mut m, R0 + field::TIME, 9.0);
        set(&mut m, R0 + field::TARGET + 4, 1.0);
        set(&mut m, R0 + field::SPEED, 2.0);
        set(&mut m, R0 + field::TARGET_SPEED, 4.0);
        set(&mut m, R0 + field::GUST_AT, 9.0);
        set(&mut m, R0 + field::GUST_PERIOD, 5.0);
        run(&mut m, MXCSR, CW).unwrap();
        // (0.5, 0.5) normalised; speed 0.5 * 2 + 0.5 * 4.
        let h = rf(&m, R0 + field::DIRECTION);
        assert!((h - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert_eq!(rf(&m, R0 + field::DIRECTION + 4), h);
        assert_eq!(rf(&m, R0 + field::SPEED), 3.0);
        assert_eq!(m.u32(RNG + 4).unwrap(), 0x1234_5678);
        assert_eq!(rf(&m, R0 + field::GUST_AT), 9.0);
    }

    #[test]
    fn a_gust_draws_a_new_target_speed_and_period() {
        let mut m = world();
        set(&mut m, DEF + 0x30, 6.0);
        set(&mut m, DEF + 0x2c, 2.0);
        set(&mut m, DEF + 0x34, 1.0);
        set(&mut m, DEF + 0x38, 3.0);
        set(&mut m, DEF + 0x3c, 0.25);
        set(&mut m, R0 + field::GUST_PERIOD, 1.0);
        run(&mut m, MXCSR, CW).unwrap();
        let mut s = 0x1234_5678u32;
        for _ in 0..4 {
            s = lcg(s);
        }
        assert_eq!(m.u32(RNG + 4).unwrap(), s);
        assert_eq!(rf(&m, R0 + field::GUST_AT), 10.0);
        let speed = rf(&m, R0 + field::TARGET_SPEED);
        assert!((2.0..=6.001).contains(&speed), "{speed}");
        let period = rf(&m, R0 + field::GUST_PERIOD);
        assert!((1.0..=3.001).contains(&period), "{period}");
        let (tx, ty) = (rf(&m, R0 + field::TARGET), rf(&m, R0 + field::TARGET + 4));
        assert!(((tx * tx + ty * ty) - 1.0).abs() < 1e-5, "{tx} {ty}");
    }

    #[test]
    fn the_inverse_takes_each_scale() {
        let mut sse = Sse(MXCSR);
        let one = f(1.0);
        let m = [2.0f32, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 2.0, 4.0, 6.0].map(f);
        let o = inverse(&mut sse, one, &m).unwrap();
        assert_eq!(o[0], f(0.5));
        assert_eq!([o[10], o[11], o[12]], [f(-1.0), f(-2.0), f(-3.0)]);
        let mut unit = m;
        unit[0] = one;
        let o = inverse(&mut sse, one, &unit).unwrap();
        assert_eq!([o[0], o[10], o[11], o[12]], [one, f(-2.0), f(-4.0), f(-6.0)]);
        let mut zero = m;
        zero[0] = f(-0.0);
        assert_eq!(inverse(&mut sse, one, &zero).unwrap(), [0; 13]);
        // The axes transposed.
        let r = [1.0f32, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 0.0, 0.0, 0.0].map(f);
        let o = inverse(&mut sse, one, &r).unwrap();
        assert_eq!(&o[1..10], &[1.0f32, 4.0, 7.0, 2.0, 5.0, 8.0, 3.0, 6.0, 9.0].map(f));
    }

    #[test]
    fn compose_applies_the_first_to_the_second() {
        let mut sse = Sse(MXCSR);
        let a = [2.0f32, 0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 10.0, 20.0, 30.0].map(f);
        let b = [3.0f32, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 2.0, 3.0].map(f);
        let o = compose(&mut sse, &a, &b).unwrap();
        assert_eq!(o[0], f(6.0));
        // Rotation (x -> y, y -> -x), scale 2, then the translation.
        assert_eq!([o[10], o[11], o[12]], [f(6.0), f(22.0), f(36.0)]);
        assert_eq!(&o[1..4], &[0.0f32, 1.0, 0.0].map(f));
        assert_eq!(&o[4..7], &[-1.0f32, 0.0, 0.0].map(f));
    }

    #[test]
    fn the_layers_without_scroll_take_their_depth() {
        let mut m = world();
        deep(&mut m);
        run(&mut m, MXCSR, CW).unwrap();
        // floor(0.5) = 0: no wrap, no new slot.
        assert_eq!(rf(&m, R0 + field::DEPTH), 0.5);
        assert_eq!(m.u32(R0 + field::PHASE).unwrap(), 0);
        assert_eq!(m.u32(RNG + 4).unwrap(), 0x1234_5678);
        for j in 0..4u32 {
            let lay = LAYERS + j * LAYER;
            // Phase j % 4, no scroll.
            assert_eq!(m.u32(lay).unwrap(), j);
            assert_eq!(m.u32(lay + 4).unwrap(), 0);
            // t = (j + 0.5) / 4.
            assert_eq!(rf(&m, lay + 0x14), (j as f32 + 0.5) * 0.25);
            assert_eq!(m.u32(lay + 8).unwrap(), 0);
            assert_eq!(m.u32(lay + 0x10).unwrap(), 0);
            let a = ((j as f64 + 0.5) * 0.5 - 1.0).abs();
            let w = (1.0 - a.powi(3)).powi(2);
            assert!((f64::from(rf(&m, lay + 0xc)) - w).abs() < 1e-6, "{j}");
            assert_eq!(m.u32(lay + 0x20).unwrap(), 0);
            assert_eq!(m.u32(lay + 0x30).unwrap(), 0);
        }
    }

    #[test]
    fn the_layers_with_scroll_take_three_weights() {
        let mut m = world();
        deep(&mut m);
        set(&mut m, DEF + 0x28, 4.0);
        set(&mut m, DEF + 0x10, 1.0);
        set(&mut m, DEF + 8, 1.0);
        run(&mut m, MXCSR, CW).unwrap();
        for i in 0..4u32 {
            let lay = LAYERS + i * LAYER;
            // time / 4 + 4 * weight: 2.5 for layer 0.
            let g = rf(&m, lay + 4);
            assert!((0.0..1.0).contains(&g), "{g}");
            let (n, mid, far) = (rf(&m, lay + 8), rf(&m, lay + 0xc), rf(&m, lay + 0x10));
            for v in [n, mid, far] {
                assert!((0.0..=1.0).contains(&v));
            }
            assert!(rf(&m, lay + 0x18) != 0.0);
        }
        assert_eq!(m.u32(LAYERS).unwrap(), 2);
        assert_eq!(rf(&m, LAYERS + 4), 0.5);
        assert_eq!(m.u8(WARNED).unwrap(), 0);
    }

    #[test]
    fn a_negative_phase_is_cleared_and_warned_once() {
        let mut m = world();
        deep(&mut m);
        set(&mut m, DEF + 0x28, 1.0);
        m.write(TIME, &(-10.5f64).to_bits().to_le_bytes()).unwrap();
        run(&mut m, MXCSR, CW).unwrap();
        assert_eq!(m.u8(WARNED).unwrap(), 1);
        assert_eq!(m.u32(LAYERS).unwrap(), 0);
        assert_eq!(rf(&m, LAYERS + 4), 0.5);
    }

    #[test]
    fn the_depth_wraps_and_draws_a_slot_either_way() {
        // Down past 0: the slot behind the phase.
        let mut m = world();
        deep(&mut m);
        set(&mut m, DEF + 0xc, 1.0);
        // The source's position is the definition's +0x44 plus the wind.
        set(&mut m, DEF + 0x4c, 1.0);
        run(&mut m, MXCSR, CW).unwrap();
        assert_eq!(rf(&m, R0 + field::DEPTH), 0.5);
        assert_eq!(m.u32(R0 + field::PHASE).unwrap(), 1);
        assert_eq!(m.u32(RNG + 4).unwrap(), lcg(lcg(0x1234_5678)));
        assert!(rf(&m, R0 + field::SLOTS) != 0.0);
        // Up past 1: the slot at the phase.
        let mut m = world();
        deep(&mut m);
        set(&mut m, DEF + 0xc, -1.0);
        // The source's position is the definition's +0x44 plus the wind.
        set(&mut m, DEF + 0x4c, 1.0);
        m.set_u32(R0 + field::PHASE, 2).unwrap();
        run(&mut m, MXCSR, CW).unwrap();
        assert_eq!(rf(&m, R0 + field::DEPTH), 0.5);
        assert_eq!(m.u32(R0 + field::PHASE).unwrap(), 1);
        assert!(rf(&m, R0 + field::SLOTS + 8) != 0.0);
    }

    #[test]
    fn refusals_name_their_cause_and_write_nothing() {
        let mut m = world();
        deep(&mut m);
        set(&mut m, DEF + 0x28, 1.0);
        m.set_u32(LAYERS_DEF + 0x44, 0).unwrap();
        let before = all(&m);
        assert_eq!(run(&mut m, MXCSR, CW), Err("idiv"));
        assert_eq!(all(&m), before);
        let mut m = world();
        deep(&mut m);
        m.set_u32(R0 + field::DEPTH, 0x7fc0_0000).unwrap();
        assert_eq!(run(&mut m, MXCSR, CW), Err("floor_nan"));
        let mut m = world();
        deep(&mut m);
        // Division by zero unmasked: the window has no width.
        m.write(WINDOW + 6, &0i16.to_le_bytes()).unwrap();
        assert_eq!(run(&mut m, MXCSR & !(simd::ZE << simd::MASK_SHIFT), CW), Err("simd"));
    }
}
