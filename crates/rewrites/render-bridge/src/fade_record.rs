//! `0x0001fd20` in Rust: the frame tail's fade record brought up to the
//! clock (docs/graphics/retire-d3d-display.md, "The frame tail").
//!
//! [`update`] makes the guest's every write in its order, its stack
//! slots included, and leaves EAX, ECX, EDX, XMM0..5, the x87 state and
//! MXCSR as the guest's body does (EFLAGS are not kept: the frame tail
//! reads none). The record `[0x005093e0]` (none, or `+0xac` clear:
//! nothing but the `push ecx`):
//!
//! | Code | Writes |
//! |---|---|
//! | `0x0001fd3d..0x0001fe88` | `+0x04` from `+0xb0`/`+0xb4`, `+0x10` from `+0xc0`/`+0xc4`, clamped to 0..1 |
//! | `..0x0001fef7` | `+0x14` from `+0xc8`/`+0xcc`; `+0x18..+0x24` replaced from `[[0x00468720]]` when equal to `[[0x00468718]]` (`repe cmpsd`) |
//! | `..0x0001ff3e` | `+0x04` and the words `+0x00`, `+0x02` zeroed at or below `[0x0045dbdc]` or `[0x004ba04c] > 1`; `+0x10`, `+0x14` zeroed when both are at or below it and the second blend is done |
//! | `..0x0002014f` | `+0x3c..+0x48` set from `+0xd8..+0xf8`; the whole block zeroed when it is spent |
//! | `..0x0002030f` | `+0x4c..+0x5c` from `+0xfc..+0x120` and the blend of `+0x124`/`+0x128`; `+0x60..+0x8c` from them, each times its colour when `+0x15c..+0x15f` is set |
//! | `..0x000204ab` | `+0x90..+0x9c` copied from `+0x160..+0x16c`; `+0xa4` (one less the blend of `+0x178`/`+0x17c`, or 0), `+0xa0` set once from `+0x170`, `+0xa8` from `+0x174` |
//!
//! Each blend `(clock - from) / (to - from)`, clamped to `[0x0045dbe0]`..
//! `[0x0045dcc0]`, is the x87's, run by the x87 crate's model under the
//! caller's control word (its precision and rounding exactly), the
//! marks (`fip`, `fop`, `fdp`) per instruction; `from == to` skips it.
//! The SSE scalar arithmetic is the Pentium III's under the caller's MXCSR
//! ([`Sse`]): NaNs propagated as x86 does, the sticky flags set. An
//! operand neither model makes exactly is `Err("x87")` or `Err("simd")`;
//! an unreadable or unwritable address `Err("memory")`.

use guest_layout::memory::{GuestMemory, MemFault};
use runtime::cpu::{CpuAbi, X87Abi};
use runtime::generated::x87_helpers as fx;

/// The fade record's pointer.
pub const RECORD: u32 = 0x0050_93e0;
/// The clock (a double).
pub const CLOCK: u32 = 0x0048_58a0;
/// One (a float), and the blends' clamp (doubles).
pub const ONE: u32 = 0x0045_dbc0;
pub const LOW: u32 = 0x0045_dbe0;
pub const HIGH: u32 = 0x0045_dcc0;
/// The floor `+0x04`, `+0x10` and `+0x14` are zeroed at.
pub const FLOOR: u32 = 0x0045_dbdc;
/// The three words `+0x18` is compared with, and the three it then takes.
pub const MATCH: u32 = 0x0046_8718;
pub const REPLACEMENT: u32 = 0x0046_8720;
/// Above 1, `+0x04` is zeroed.
pub const COUNT: u32 = 0x004b_a04c;

type Cause = &'static str;

fn fault(_: MemFault) -> Cause {
    "memory"
}

const IE: u32 = 0x01;
const DE: u32 = 0x02;
const OE: u32 = 0x08;
const PE: u32 = 0x20;
/// The x86 default NaN (negative quiet).
const DEFAULT_NAN: u32 = 0xffc0_0000;
const QUIET: u32 = 0x0040_0000;

fn nan(b: u32) -> bool {
    b & 0x7fff_ffff > 0x7f80_0000
}
fn snan(b: u32) -> bool {
    nan(b) && b & QUIET == 0
}
fn denormal(b: u32) -> bool {
    b & 0x7f80_0000 == 0 && b & 0x007f_ffff != 0
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Add,
    Sub,
    Mul,
}

/// SSE scalar arithmetic as the Pentium III makes it: round to nearest,
/// every exception masked, no flush to zero (any other MXCSR is
/// `simd`). A NaN operand gives the first NaN quieted (IE for a
/// signalling one), an invalid operation the default NaN with IE,
/// overflow OE and PE, an inexact result PE, a denormal operand DE.
/// Refused: a NaN with a denormal beside it, and a tiny inexact product
/// (where x86 and the host differ on tininess).
pub(crate) struct Sse {
    pub(crate) mxcsr: u32,
}

impl Sse {
    fn mode(&self) -> Result<(), Cause> {
        if self.mxcsr & 0xffc0 == 0x1f80 {
            Ok(())
        } else {
            Err("simd")
        }
    }

    fn op(&mut self, op: Op, a: u32, b: u32) -> Result<u32, Cause> {
        self.mode()?;
        if nan(a) || nan(b) {
            if denormal(a) || denormal(b) {
                return Err("simd");
            }
            if snan(a) || snan(b) {
                self.mxcsr |= IE;
            }
            return Ok((if nan(a) { a } else { b }) | QUIET);
        }
        if denormal(a) || denormal(b) {
            self.mxcsr |= DE;
        }
        let (x, y) = (f32::from_bits(a), f32::from_bits(b));
        let r = match op {
            Op::Add => x + y,
            Op::Sub => x - y,
            Op::Mul => x * y,
        };
        if r.is_nan() {
            self.mxcsr |= IE;
            return Ok(DEFAULT_NAN);
        }
        if !x.is_finite() || !y.is_finite() {
            return Ok(r.to_bits());
        }
        if r.is_infinite() {
            self.mxcsr |= OE | PE;
            return Ok(r.to_bits());
        }
        let inexact = match op {
            Op::Mul => {
                // Exact: two 24-bit significands fit a double's 53.
                let p = f64::from(x) * f64::from(y);
                let inexact = f64::from(r) != p;
                if inexact && p.abs() < f64::from(f32::MIN_POSITIVE) {
                    return Err("simd");
                }
                inexact
            }
            Op::Add | Op::Sub => {
                // TwoSum: the rounding error, exact with no overflow. A
                // sum below the normal range is always exact.
                let y = if op == Op::Sub { -y } else { y };
                let bb = r - x;
                (x - (r - bb)) + (y - bb) != 0.0
            }
        };
        if inexact {
            self.mxcsr |= PE;
        }
        Ok(r.to_bits())
    }

    pub(crate) fn add(&mut self, a: u32, b: u32) -> Result<u32, Cause> {
        self.op(Op::Add, a, b)
    }
    pub(crate) fn sub(&mut self, a: u32, b: u32) -> Result<u32, Cause> {
        self.op(Op::Sub, a, b)
    }
    pub(crate) fn mul(&mut self, a: u32, b: u32) -> Result<u32, Cause> {
        self.op(Op::Mul, a, b)
    }

    /// `comiss` (`quiet` false) or `ucomiss`: ZF `0x40`, PF `0x04`, CF
    /// `0x01`.
    fn compare(&mut self, a: u32, b: u32, quiet: bool) -> Result<u32, Cause> {
        self.mode()?;
        if nan(a) || nan(b) {
            if denormal(a) || denormal(b) {
                return Err("simd");
            }
            if !quiet || snan(a) || snan(b) {
                self.mxcsr |= IE;
            }
            return Ok(0x45);
        }
        if denormal(a) || denormal(b) {
            self.mxcsr |= DE;
        }
        let (x, y) = (f32::from_bits(a), f32::from_bits(b));
        Ok(if x > y {
            0
        } else if x < y {
            0x01
        } else {
            0x40
        })
    }
    /// `comiss a, b; ja`.
    pub(crate) fn above(&mut self, a: u32, b: u32) -> Result<bool, Cause> {
        Ok(self.compare(a, b, false)? & 0x41 == 0)
    }
    /// `comiss a, b; jb`.
    pub(crate) fn below(&mut self, a: u32, b: u32) -> Result<bool, Cause> {
        Ok(self.compare(a, b, false)? & 0x01 != 0)
    }
    /// `ucomiss a, b; lahf; test ah, 44h; jnp`: ordered and equal.
    pub(crate) fn equal(&mut self, a: u32, b: u32) -> Result<bool, Cause> {
        Ok(self.compare(a, b, true)? & 0x44 == 0x40)
    }
    /// `lo` if `lo > v`, else `hi` if `v > hi`, else `v`.
    pub(crate) fn clamp(&mut self, v: u32, lo: u32, hi: u32) -> Result<u32, Cause> {
        Ok(if self.above(lo, v)? {
            lo
        } else if self.above(v, hi)? {
            hi
        } else {
            v
        })
    }
}

/// The x87 through the x87 crate's entries (`h2x87_v1_*`, the ones the
/// lifted C calls), on the caller's state.
struct Fpu {
    s: X87Abi,
}

/// The entries' status byte.
const STATUS_MASK: u32 = 0xff00_0000;
/// `0x0001fd20`'s x87 ops (`fop`).
const FLD_M32: u16 = 0x182;
const FLD_M64: u16 = 0x505;
const FSUBR_M64: u16 = 0x42d;
const FSUB_M32: u16 = 0x0a2;
const FDIVP: u16 = 0x6f9;
const FLD_ST: u16 = 0x1c0;
const FCOMIP: u16 = 0x7f1;
const FSTP_ST0: u16 = 0x5d8;
const FSTP_M32: u16 = 0x11c;
const GUEST_CS: u16 = 0x0008;
const GUEST_DS: u16 = 0x0010;

impl Fpu {
    /// The runtime's `X87Abi` is the x87 crate's, field for field (the
    /// lifted C hands `&cpu->x87` to the same entries).
    fn p(&mut self) -> *mut X87Abi {
        &mut self.s
    }
    fn ok(code: u32) -> Result<u32, Cause> {
        if code & STATUS_MASK == 0 {
            Ok(code)
        } else {
            Err("x87")
        }
    }
    /// `fwait` before a memory operand: a pending exception stops it.
    fn wait(&self) -> Result<(), Cause> {
        if self.s.status & 0x80 != 0 {
            Err("x87")
        } else {
            Ok(())
        }
    }
    fn mark(&mut self, pc: u32, fop: u16, ea: Option<u32>) {
        self.s.instruction_pointer = pc;
        self.s.opcode = fop;
        self.s.instruction_selector = GUEST_CS;
        if let Some(ea) = ea {
            self.s.data_pointer = ea;
            self.s.data_selector = GUEST_DS;
        }
    }
    fn fld_m32(&mut self, pc: u32, ea: u32, bits: u32) -> Result<(), Cause> {
        self.wait()?;
        // SAFETY: `p` is this state, live and exclusive for the call.
        Self::ok(unsafe { fx::h2x87_v1_ld_f32(self.p().cast(), bits) })?;
        self.mark(pc, FLD_M32, Some(ea));
        Ok(())
    }
    fn fld_m64(&mut self, pc: u32, ea: u32, bits: u64) -> Result<(), Cause> {
        self.wait()?;
        // SAFETY: as above.
        Self::ok(unsafe { fx::h2x87_v1_ld_f64(self.p().cast(), bits) })?;
        self.mark(pc, FLD_M64, Some(ea));
        Ok(())
    }
    /// `fsubr st, qword` (`SUBR`, an `F64` source).
    fn fsubr_m64(&mut self, pc: u32, ea: u32, bits: u64) -> Result<(), Cause> {
        self.wait()?;
        // SAFETY: as above.
        Self::ok(unsafe { fx::h2x87_v1_arith(self.p().cast(), 2, 0, 2, bits) })?;
        self.mark(pc, FSUBR_M64, Some(ea));
        Ok(())
    }
    /// `fsub st, dword` (`SUB`, an `F32` source).
    fn fsub_m32(&mut self, pc: u32, ea: u32, bits: u32) -> Result<(), Cause> {
        self.wait()?;
        // SAFETY: as above.
        Self::ok(unsafe { fx::h2x87_v1_arith(self.p().cast(), 1, 0, 1, u64::from(bits)) })?;
        self.mark(pc, FSUB_M32, Some(ea));
        Ok(())
    }
    /// `fdivp st(1), st` (`DIV | ARITH_POP` into st(1)).
    fn fdivp(&mut self, pc: u32) -> Result<(), Cause> {
        // SAFETY: as above.
        Self::ok(unsafe { fx::h2x87_v1_arith(self.p().cast(), 0x104, 1, 0, 0) })?;
        self.mark(pc, FDIVP, None);
        Ok(())
    }
    fn fld_st(&mut self, pc: u32, i: u32) -> Result<(), Cause> {
        // SAFETY: as above.
        Self::ok(unsafe { fx::h2x87_v1_ld_st(self.p().cast(), i) })?;
        self.mark(pc, FLD_ST | i as u16, None);
        Ok(())
    }
    /// `fcomip st, st(1)`: ZF `0x40`, PF `0x04`, CF `0x01`. An unmasked
    /// exception (the flags kept) is `x87`.
    fn fcomip(&mut self, pc: u32) -> Result<u32, Cause> {
        // SAFETY: as above.
        let r = Self::ok(unsafe { fx::h2x87_v1_cmp(self.p().cast(), 0, 0, 1, 1, 1) })?;
        self.mark(pc, FCOMIP, None);
        Ok(r & 0x45)
    }
    fn fstp_st0(&mut self, pc: u32) -> Result<(), Cause> {
        // SAFETY: as above.
        Self::ok(unsafe { fx::h2x87_v1_st_st(self.p().cast(), 0, 1) })?;
        self.mark(pc, FSTP_ST0, None);
        Ok(())
    }
}

struct Run<'a> {
    mem: &'a mut dyn GuestMemory,
    edx: u32,
    /// `[esp]` inside the body: the `push ecx` slot.
    scratch: u32,
    sse: Sse,
    fpu: Fpu,
}

impl Run<'_> {
    fn at(&self, addr: u32) -> Result<u32, Cause> {
        self.mem.u32(addr).map_err(fault)
    }
    fn q(&self, addr: u32) -> Result<u64, Cause> {
        Ok(u64::from(self.at(addr)?) | u64::from(self.at(addr.wrapping_add(4))?) << 32)
    }
    fn r(&self, off: u32) -> Result<u32, Cause> {
        self.at(self.edx.wrapping_add(off))
    }
    fn byte(&self, off: u32) -> Result<u8, Cause> {
        self.mem.u8(self.edx.wrapping_add(off)).map_err(fault)
    }
    fn w(&mut self, off: u32, v: u32) -> Result<(), Cause> {
        self.mem
            .write(self.edx.wrapping_add(off), &v.to_le_bytes())
            .map_err(fault)
    }
    fn w16(&mut self, off: u32, v: u16) -> Result<(), Cause> {
        self.mem
            .write(self.edx.wrapping_add(off), &v.to_le_bytes())
            .map_err(fault)
    }
    fn wb(&mut self, off: u32, v: u8) -> Result<(), Cause> {
        self.mem
            .write(self.edx.wrapping_add(off), &[v])
            .map_err(fault)
    }

    /// `fstp dword [esp]`: the store, its write, then its commit.
    fn fstp_scratch(&mut self, pc: u32) -> Result<(), Cause> {
        self.fpu.wait()?;
        let (mut v, mut sw) = (0u32, 0u16);
        // SAFETY: the state and both outs are live locals.
        Fpu::ok(unsafe { fx::h2x87_v1_st_f32(self.fpu.p().cast(), 0, &mut v, &mut sw) })?;
        self.mem
            .write(self.scratch, &v.to_le_bytes())
            .map_err(fault)?;
        // SAFETY: as above.
        Fpu::ok(unsafe { fx::h2x87_v1_store_commit(self.fpu.p().cast(), sw, 1) })?;
        self.fpu.mark(pc, FSTP_M32, Some(self.scratch));
        Ok(())
    }

    /// The blend block at `base`: `(clock - [+y]) / ([+x] - [+y])`,
    /// clamped to `LOW..HIGH`, stored to `[esp]` and read back. `shared`:
    /// the layout whose two clamps end in one store (`0x0001ff96`,
    /// `0x0001fffb`, `0x00020167`, `0x00020413`), not one each
    /// (`0x0001fd5a`, `0x0001fdcc`).
    fn blend(&mut self, base: u32, shared: bool, x: u32, y: u32) -> Result<u32, Cause> {
        let (ex, ey) = (self.edx.wrapping_add(x), self.edx.wrapping_add(y));
        let v = self.at(ey)?;
        self.fpu.fld_m32(base, ey, v)?;
        let t = self.q(CLOCK)?;
        self.fpu.fsubr_m64(base + 0x6, CLOCK, t)?;
        let v = self.at(ex)?;
        self.fpu.fld_m32(base + 0xc, ex, v)?;
        let v = self.at(ey)?;
        self.fpu.fsub_m32(base + 0x12, ey, v)?;
        self.fpu.fdivp(base + 0x18)?;
        self.fpu.fld_st(base + 0x1a, 0)?;
        let lo = self.q(LOW)?;
        self.fpu.fld_m64(base + 0x1c, LOW, lo)?;
        let f = self.fpu.fcomip(base + 0x22)?;
        self.fpu.fstp_st0(base + 0x24)?;
        // `jbe`: CF or ZF.
        let store = if f & 0x41 == 0 {
            self.fpu.fstp_st0(base + 0x28)?;
            let lo = self.q(LOW)?;
            self.fpu.fld_m64(base + 0x2a, LOW, lo)?;
            if shared { base + 0x48 } else { base + 0x30 }
        } else {
            let m = if shared { base + 0x32 } else { base + 0x3a };
            let hi = self.q(HIGH)?;
            self.fpu.fld_m64(m, HIGH, hi)?;
            self.fpu.fld_st(m + 0x6, 1)?;
            let f = self.fpu.fcomip(m + 0x8)?;
            self.fpu.fstp_st0(m + 0xa)?;
            if f & 0x41 == 0 {
                self.fpu.fstp_st0(m + 0xe)?;
                let hi = self.q(HIGH)?;
                self.fpu.fld_m64(m + 0x10, HIGH, hi)?;
            }
            if shared { base + 0x48 } else { base + 0x50 }
        };
        self.fstp_scratch(store)?;
        self.at(self.scratch)
    }

    /// `fld dword [edx+off]; fld qword [clock]; fcomip; fstp st(0)` at
    /// `pc`: the clock below `[+off]` (CF).
    fn clock_below(&mut self, pc: u32, off: u32) -> Result<bool, Cause> {
        let ea = self.edx.wrapping_add(off);
        let v = self.at(ea)?;
        self.fpu.fld_m32(pc, ea, v)?;
        let t = self.q(CLOCK)?;
        self.fpu.fld_m64(pc + 0x6, CLOCK, t)?;
        let f = self.fpu.fcomip(pc + 0xc)?;
        self.fpu.fstp_st0(pc + 0xe)?;
        Ok(f & 0x01 != 0)
    }

    /// `0x0001fd3d..0x000204ab`: XMM0..5 at the end.
    fn body(&mut self, cpu: &CpuAbi) -> Result<[u32; 6], Cause> {
        let zero = 0u32;
        // 0x0001fd3d: the first blend.
        let (bc, b8) = (self.r(0xbc)?, self.r(0xb8)?);
        let eq = self.sse.equal(bc, b8)?;
        let one = self.at(ONE)?;
        let mut x0 = if eq {
            one
        } else {
            self.blend(0x0001_fd5a, false, 0xbc, 0xb8)?
        };
        // 0x0001fdb7: the second.
        let (d4, d0) = (self.r(0xd4)?, self.r(0xd0)?);
        let mut x1;
        let mut x4 = if self.sse.equal(d4, d0)? {
            one
        } else {
            self.blend(0x0001_fdcc, false, 0xd4, 0xd0)?
        };
        // 0x0001fe29: +0x04.
        let mut x2 = self.sse.mul(self.r(0xb4)?, x0)?;
        x1 = self.sse.sub(one, x0)?;
        x1 = self.sse.mul(x1, self.r(0xb0)?)?;
        x1 = self.sse.add(x1, x2)?;
        self.w(0x04, x1)?;
        // 0x0001fe4d: +0x10.
        x2 = self.r(0xc4)?;
        let mut x5 = self.r(0xc0)?;
        x0 = self.sse.sub(one, x4)?;
        x2 = self.sse.mul(x2, x4)?;
        x1 = self.sse.mul(x0, x5)?;
        x1 = self.sse.add(x1, x2)?;
        x2 = zero;
        x1 = self.sse.clamp(x1, zero, one)?;
        self.w(0x10, x1)?;
        // 0x0001fe8d: +0x14.
        x1 = self.r(0xcc)?;
        x5 = self.r(0xc8)?;
        x0 = self.sse.mul(x0, x5)?;
        x1 = self.sse.mul(x1, x4)?;
        x0 = self.sse.add(x0, x1)?;
        x0 = self.sse.clamp(x0, zero, one)?;
        // 0x0001febb: push ebp, esi, edi.
        let e = self.scratch;
        for (i, v) in [cpu.ebp, cpu.esi, cpu.edi].into_iter().enumerate() {
            self.mem
                .write(e.wrapping_sub(4 * (i as u32 + 1)), &v.to_le_bytes())
                .map_err(fault)?;
        }
        let key = self.at(MATCH)?;
        self.w(0x14, x0)?;
        // 0x0001fed5: repe cmpsd over three words.
        let mut same = true;
        for i in 0..3 {
            let a = self.r(0x18 + 4 * i)?;
            let b = self.at(key.wrapping_add(4 * i))?;
            if a != b {
                same = false;
                break;
            }
        }
        if same {
            let src = self.at(REPLACEMENT)?;
            for i in 0..3 {
                let v = self.at(src.wrapping_add(4 * i))?;
                self.w(0x18 + 4 * i, v)?;
            }
        }
        // 0x0001feef: the floor.
        x0 = self.at(FLOOR)?;
        let at_floor = !self.sse.below(x0, self.r(0x04)?)?;
        if self.mem.i32(COUNT).map_err(fault)? > 1 || at_floor {
            self.w(0x04, x2)?;
            self.w16(0x02, 0)?;
            self.w16(0x00, 0)?;
        }
        if !self.sse.below(x0, self.r(0x10)?)?
            && !self.sse.below(x0, self.r(0x14)?)?
            && !self.sse.below(x4, one)?
        {
            self.w(0x10, x2)?;
            self.w(0x14, x2)?;
        }
        // 0x0001ff3e: the four weights; none above 0 is spent.
        let mut spent = true;
        for off in [0xdc, 0xe0, 0xec, 0xf0] {
            x0 = self.r(off)?;
            if self.sse.above(x0, x2)? {
                spent = false;
                break;
            }
        }
        if !spent {
            // 0x0001ff7b.
            x0 = self.r(0xe8)?;
            let e4 = self.r(0xe4)?;
            let eq = self.sse.equal(x0, e4)?;
            x1 = one;
            x4 = one;
            if !eq {
                x1 = self.blend(0x0001_ff96, true, 0xe8, 0xe4)?;
            }
            x0 = self.r(0xf8)?;
            if !self.sse.equal(x0, self.r(0xf4)?)? {
                x4 = self.blend(0x0001_fffb, true, 0xf8, 0xf4)?;
            }
            x5 = self.sse.mul(self.r(0xe0)?, x1)?;
            x0 = self.sse.sub(one, x1)?;
            x0 = self.sse.mul(x0, self.r(0xdc)?)?;
            x0 = self.sse.add(x0, x5)?;
            let first = self.sse.above(x0, x2)?;
            x5 = self.r(0xf0)?;
            x1 = self.sse.sub(one, x4)?;
            x1 = self.sse.mul(x1, self.r(0xec)?)?;
            x5 = self.sse.mul(x5, x4)?;
            x1 = self.sse.add(x1, x5)?;
            if first || self.sse.above(x1, x2)? {
                let d8 = self.r(0xd8)?;
                self.wb(0x3c, 1)?;
                self.w(0x40, d8)?;
                self.w(0x44, x0)?;
                self.w(0x48, x1)?;
            }
        }
        // 0x000200aa: both ends 0 and the clock past both times also
        // spends it.
        x0 = self.r(0xe0)?;
        let mut clear = spent;
        if self.sse.equal(x0, x2)? {
            x0 = self.r(0xf0)?;
            if self.sse.equal(x0, x2)?
                && !self.clock_below(0x0002_00cc, 0xe8)?
                && !self.clock_below(0x0002_00de, 0xf8)?
            {
                clear = true;
            }
        }
        if clear {
            self.wb(0x3c, 0)?;
            for off in [
                0x40, 0x44, 0x48, 0xd8, 0xdc, 0xe0, 0xe4, 0xe8, 0xec, 0xf0, 0xf4, 0xf8,
            ] {
                self.w(off, x2)?;
            }
        }
        // 0x0002014f: the colour blend.
        x0 = self.r(0x128)?;
        x1 = x2;
        if !self.sse.equal(x0, self.r(0x124)?)? {
            x1 = self.blend(0x0002_0167, true, 0x128, 0x124)?;
        }
        x4 = self.r(0xfc)?;
        x5 = self.r(0x100)?;
        x5 = self.sse.mul(x5, x1)?;
        x0 = self.sse.sub(one, x1)?;
        x4 = self.sse.mul(x4, x0)?;
        x4 = self.sse.add(x4, x5)?;
        self.w(0x4c, x4)?;
        for (dst, lo, hi) in [
            (0x50, 0x104, 0x108),
            (0x54, 0x10c, 0x110),
            (0x58, 0x114, 0x118),
        ] {
            x4 = self.r(lo)?;
            x5 = self.r(hi)?;
            x4 = self.sse.mul(x4, x0)?;
            x5 = self.sse.mul(x5, x1)?;
            x4 = self.sse.add(x4, x5)?;
            self.w(dst, x4)?;
        }
        x4 = self.r(0x11c)?;
        x4 = self.sse.mul(x4, x0)?;
        x0 = self.r(0x120)?;
        x0 = self.sse.mul(x0, x1)?;
        x4 = self.sse.add(x4, x0)?;
        self.w(0x5c, x4)?;
        // 0x00020263: the four colours, each its scale times its colour
        // when the flag is set, else the scale three times.
        for (flag, src, dst, colour) in [
            (0x15c, 0x50, 0x60, 0x12c),
            (0x15d, 0x54, 0x6c, 0x138),
            (0x15e, 0x58, 0x78, 0x144),
            (0x15f, 0x5c, 0x84, 0x150),
        ] {
            let set = self.byte(flag)? != 0;
            x0 = self.r(src)?;
            for i in 0..3 {
                let v = if set {
                    x1 = self.sse.mul(self.r(colour + 4 * i)?, x0)?;
                    x1
                } else {
                    x0
                };
                self.w(dst + 4 * i, v)?;
            }
        }
        // 0x000203c7: the copies and the fade.
        let eax = self.r(0x164)?;
        let ecx = self.r(0x160)?;
        x1 = self.r(0x17c)?;
        let eq = self.sse.equal(x1, self.r(0x178)?)?;
        self.w(0x94, eax)?;
        let eax = self.r(0x16c)?;
        self.w(0x9c, eax)?;
        self.w(0x90, ecx)?;
        let ecx = self.r(0x168)?;
        self.w(0x98, ecx)?;
        x0 = x2;
        if !eq {
            let s = self.blend(0x0002_0413, true, 0x17c, 0x178)?;
            x0 = self.sse.sub(one, s)?;
            let up = if self.sse.above(x2, x0)? {
                x0 = x2;
                false
            } else if self.sse.above(x0, one)? {
                x0 = one;
                true
            } else {
                self.sse.above(x0, x2)?
            };
            if up && self.byte(0x170)? != 0 {
                self.wb(0xa0, 1)?;
                self.wb(0x170, 0)?;
            }
        }
        let ecx = self.r(0x174)?;
        self.w(0xa4, x0)?;
        self.w(0xa8, ecx)?;
        Ok([x0, x1, x2, one, x4, x5])
    }
}

/// `0x0001fd20` on `cpu` at its entry (ESP at the return address): the
/// guest's writes, its registers at `ret` (EAX, ECX, EDX, XMM0..5, the
/// x87 state, MXCSR) and EAX, the record or the EAX it came in with.
pub fn update(mem: &mut dyn GuestMemory, cpu: &mut CpuAbi) -> Result<u32, Cause> {
    let scratch = cpu.esp.wrapping_sub(4);
    mem.write(scratch, &cpu.ecx.to_le_bytes()).map_err(fault)?;
    let edx = mem.u32(RECORD).map_err(fault)?;
    cpu.edx = edx;
    if edx == 0 || mem.u8(edx.wrapping_add(0xac)).map_err(fault)? == 0 {
        return Ok(cpu.eax);
    }
    let mut run = Run {
        mem,
        edx,
        scratch,
        sse: Sse { mxcsr: cpu.mxcsr },
        fpu: Fpu { s: cpu.x87 },
    };
    let xmm = run.body(cpu)?;
    for (n, v) in xmm.into_iter().enumerate() {
        cpu.xmm[n] = u128::from(v).to_le_bytes();
    }
    cpu.x87 = run.fpu.s;
    cpu.mxcsr = run.sse.mxcsr;
    // `pop ecx`: the slot the stores last wrote.
    cpu.ecx = run.at(scratch)?;
    cpu.eax = edx;
    Ok(edx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    const AT: u32 = 0x0050_0000;
    const ESP: u32 = 0x0060_0000;
    const KEY: u32 = 0x0058_0000;
    const NEW: u32 = 0x0058_0100;
    const F: fn(f32) -> u32 = f32::to_bits;

    fn put(m: &mut FlatMemory, off: u32, v: f32) {
        m.set_u32(AT + off, F(v)).unwrap();
    }
    fn get(m: &FlatMemory, off: u32) -> f32 {
        m.f32(AT + off).unwrap()
    }
    fn clock(m: &mut FlatMemory, t: f64) {
        m.write(CLOCK, &t.to_le_bytes()).unwrap();
    }

    /// A live record, every pair equal (so no blend runs), the clock 0.
    fn world() -> FlatMemory {
        let mut m = FlatMemory::new(0x0040_0000, 0x0030_0000);
        m.set_u32(RECORD, AT).unwrap();
        m.write(AT + 0xac, &[1]).unwrap();
        m.set_u32(ONE, F(1.0)).unwrap();
        m.write(LOW, &0.0f64.to_le_bytes()).unwrap();
        m.write(HIGH, &1.0f64.to_le_bytes()).unwrap();
        m.set_u32(FLOOR, F(0.001)).unwrap();
        m.set_u32(MATCH, KEY).unwrap();
        m.set_u32(REPLACEMENT, NEW).unwrap();
        // A live weight, so the clock check at 0x000200cc does not run.
        m.set_u32(AT + 0xe0, F(0.5)).unwrap();
        for i in 0..3 {
            m.set_u32(KEY + 4 * i, 0xdead_0000 + i).unwrap();
            m.set_u32(NEW + 4 * i, 0xbeef_0000 + i).unwrap();
        }
        m
    }

    fn cpu(control: u16) -> CpuAbi {
        let mut c = CpuAbi::default();
        (c.esp, c.ecx, c.ebp, c.esi, c.edi) = (ESP, 0x1111, 0x2222, 0x3333, 0x4444);
        c.x87.control = control;
        c
    }

    fn run(m: &mut FlatMemory, c: &mut CpuAbi) -> Result<u32, Cause> {
        update(m, c)
    }

    #[test]
    fn no_record_or_a_dead_one_only_pushes_ecx() {
        let mut m = world();
        m.set_u32(RECORD, 0).unwrap();
        let mut c = cpu(0x027f);
        c.eax = 7;
        assert_eq!(run(&mut m, &mut c), Ok(7));
        assert_eq!((m.u32(ESP - 4).unwrap(), c.ecx, c.edx), (0x1111, 0x1111, 0));
        m.set_u32(RECORD, AT).unwrap();
        m.write(AT + 0xac, &[0]).unwrap();
        put(&mut m, 0xb4, 5.0);
        let before = c;
        assert_eq!(run(&mut m, &mut c), Ok(7));
        assert_eq!(m.u32(AT + 0x04).unwrap(), 0, "nothing written");
        assert_eq!((c.x87, c.xmm, c.edx), (before.x87, before.xmm, AT));
    }

    /// Every pair equal: the blends are one, no x87 runs, and the stack
    /// slot keeps ECX.
    #[test]
    fn equal_pairs_take_one_and_skip_the_x87() {
        let mut m = world();
        put(&mut m, 0xb0, 10.0);
        put(&mut m, 0xb4, 20.0);
        put(&mut m, 0xc4, 0.25);
        put(&mut m, 0xcc, 0.75);
        put(&mut m, 0x174, 2.0);
        let mut c = cpu(0x027f);
        let x87 = c.x87;
        assert_eq!(run(&mut m, &mut c), Ok(AT));
        assert_eq!(
            (get(&m, 0x04), get(&m, 0x10), get(&m, 0x14)),
            (20.0, 0.25, 0.75)
        );
        assert_eq!((get(&m, 0xa4), get(&m, 0xa8)), (0.0, 2.0));
        assert_eq!(c.x87, x87, "no x87 instruction ran");
        assert_eq!((c.ecx, c.eax, c.edx), (0x1111, AT, AT));
        assert_eq!(
            [
                m.u32(ESP - 8).unwrap(),
                m.u32(ESP - 12).unwrap(),
                m.u32(ESP - 16).unwrap()
            ],
            [0x2222, 0x3333, 0x4444]
        );
        assert_eq!(c.xmm[3], u128::from(F(1.0)).to_le_bytes());
        assert_eq!(c.mxcsr, 0x1f80, "all exact");
    }

    /// `(clock - from) / (to - from)` under each precision control: 0.5
    /// at clock 2 from 1 to 3; the x87 stack is left as it came, its
    /// marks at the last store.
    #[test]
    fn a_blend_runs_on_the_x87_at_every_precision() {
        for control in [0x007f, 0x027f, 0x037f] {
            let mut m = world();
            clock(&mut m, 2.0);
            put(&mut m, 0xb8, 1.0);
            put(&mut m, 0xbc, 3.0);
            put(&mut m, 0xb0, 10.0);
            put(&mut m, 0xb4, 20.0);
            let mut c = cpu(control);
            assert_eq!(run(&mut m, &mut c), Ok(AT));
            assert_eq!(get(&m, 0x04), 15.0, "control {control:#x}");
            assert_eq!(f32::from_bits(c.ecx), 0.5, "pop ecx takes the stored blend");
            assert_eq!((c.x87.tag, c.x87.status & 0x3800), (0xffff, 0));
            assert_eq!(
                (c.x87.instruction_pointer, c.x87.opcode, c.x87.data_pointer),
                (0x0001_fdaa, FSTP_M32, ESP - 4)
            );
        }
    }

    /// The clamps: below `[0x0045dbe0]` and above `[0x0045dcc0]`, each
    /// with its own store.
    #[test]
    fn a_blend_is_clamped_both_ways() {
        for (t, want, pc) in [(0.0, 10.0, 0x0001_fd8a), (5.0, 20.0, 0x0001_fdaa)] {
            let mut m = world();
            clock(&mut m, t);
            put(&mut m, 0xb8, 1.0);
            put(&mut m, 0xbc, 3.0);
            put(&mut m, 0xb0, 10.0);
            put(&mut m, 0xb4, 20.0);
            let mut c = cpu(0x027f);
            run(&mut m, &mut c).unwrap();
            assert_eq!(get(&m, 0x04), want);
            assert_eq!(c.x87.instruction_pointer, pc);
        }
    }

    /// A NaN pair is unordered: the blend runs, the x87's compare flags
    /// IE (masked), and the NaN reaches the record through the SSE.
    #[test]
    fn an_unordered_pair_blends_to_nan() {
        let mut m = world();
        clock(&mut m, 2.0);
        m.set_u32(AT + 0xbc, 0x7fc0_0000).unwrap();
        put(&mut m, 0xb4, 1.0);
        let mut c = cpu(0x027f);
        run(&mut m, &mut c).unwrap();
        assert!(get(&m, 0x04).is_nan());
        assert_eq!(c.x87.status & 1, 1, "fcomip on a NaN");
        assert_eq!(c.mxcsr & IE, IE, "comiss on a NaN");
        assert_eq!(c.x87.tag, 0xffff);
    }

    /// An MXCSR or control word the models cannot follow is refused.
    #[test]
    fn unmasked_exceptions_are_refused() {
        let mut m = world();
        let mut c = cpu(0x027f);
        c.mxcsr = 0x1f00;
        assert_eq!(run(&mut m, &mut c), Err("simd"));
        let mut m = world();
        clock(&mut m, 1.0);
        put(&mut m, 0xbc, 3.0);
        let mut c = cpu(0x025f);
        assert_eq!(
            run(&mut m, &mut c),
            Err("x87"),
            "1/3 is inexact, PE unmasked"
        );
    }

    #[test]
    fn the_three_words_are_replaced_only_when_they_match() {
        let mut m = world();
        for i in 0..3 {
            m.set_u32(AT + 0x18 + 4 * i, 0xdead_0000 + i).unwrap();
        }
        run(&mut m, &mut cpu(0x027f)).unwrap();
        assert_eq!(m.u32(AT + 0x20).unwrap(), 0xbeef_0002);
        let mut m = world();
        for i in 0..3 {
            m.set_u32(AT + 0x18 + 4 * i, 0xdead_0000 + i).unwrap();
        }
        m.set_u32(AT + 0x20, 1).unwrap();
        run(&mut m, &mut cpu(0x027f)).unwrap();
        assert_eq!(
            [m.u32(AT + 0x18).unwrap(), m.u32(AT + 0x20).unwrap()],
            [0xdead_0000, 1]
        );
    }

    /// `+0x04` above the floor stays unless `[0x004ba04c] > 1`; the
    /// words before it are zeroed with it.
    #[test]
    fn the_floor_and_the_count_zero_the_first_fade() {
        let mut m = world();
        put(&mut m, 0xb4, 0.5);
        m.set_u32(AT, 0x0003_0004).unwrap();
        run(&mut m, &mut cpu(0x027f)).unwrap();
        assert_eq!((get(&m, 0x04), m.u32(AT).unwrap()), (0.5, 0x0003_0004));
        m.set_i32(COUNT, 2).unwrap();
        run(&mut m, &mut cpu(0x027f)).unwrap();
        assert_eq!((get(&m, 0x04), m.u32(AT).unwrap()), (0.0, 0));
    }

    /// A weight above 0 sets `+0x3c..+0x48`; the block is kept while a
    /// weight is not 0.
    #[test]
    fn a_live_weight_sets_the_second_block() {
        let mut m = world();
        put(&mut m, 0xdc, 0.25);
        put(&mut m, 0xe0, 0.5);
        put(&mut m, 0xf0, 0.75);
        m.set_u32(AT + 0xd8, 0x1234).unwrap();
        run(&mut m, &mut cpu(0x027f)).unwrap();
        assert_eq!(
            (m.u8(AT + 0x3c).unwrap(), m.u32(AT + 0x40).unwrap()),
            (1, 0x1234)
        );
        assert_eq!((get(&m, 0x44), get(&m, 0x48)), (0.5, 0.75));
        assert_eq!(get(&m, 0xdc), 0.25, "kept");
    }

    /// Both end weights 0: the block is spent once the clock reaches both
    /// times (`fcomip` on the x87), and kept before.
    #[test]
    fn the_clock_spends_the_second_block() {
        for (t, spent) in [(1.0, false), (5.0, true)] {
            let mut m = world();
            clock(&mut m, t);
            put(&mut m, 0xe0, 0.0);
            put(&mut m, 0xdc, 0.25);
            put(&mut m, 0xe8, 2.0);
            put(&mut m, 0xe4, 2.0);
            put(&mut m, 0xf8, 3.0);
            put(&mut m, 0xf4, 3.0);
            let mut c = cpu(0x027f);
            run(&mut m, &mut c).unwrap();
            assert_eq!(get(&m, 0xdc) == 0.0, spent, "clock {t}");
            assert_eq!(c.x87.tag, 0xffff);
            let last = if t < 2.0 { 0x0002_00da } else { 0x0002_00ec };
            assert_eq!(c.x87.instruction_pointer, last);
        }
    }

    /// The colour blend and the flagged colours.
    #[test]
    fn the_colours_blend_and_scale() {
        let mut m = world();
        clock(&mut m, 2.0);
        put(&mut m, 0x124, 0.0);
        put(&mut m, 0x128, 4.0);
        put(&mut m, 0x104, 8.0);
        put(&mut m, 0x108, 16.0);
        put(&mut m, 0x12c, 0.5);
        m.write(AT + 0x15c, &[1]).unwrap();
        put(&mut m, 0x10c, 3.0);
        run(&mut m, &mut cpu(0x027f)).unwrap();
        assert_eq!(get(&m, 0x50), 12.0, "8 and 16 at a half");
        assert_eq!((get(&m, 0x60), get(&m, 0x64)), (6.0, 0.0));
        assert_eq!(
            (get(&m, 0x54), get(&m, 0x6c), get(&m, 0x74)),
            (1.5, 1.5, 1.5)
        );
    }

    /// The fade is one less the blend, clamped; above 0 it sets `+0xa0`
    /// once from `+0x170`.
    #[test]
    fn the_fade_sets_the_copy_flag_once() {
        for (t, fade, flag) in [(2.0, 0.75, 1), (0.0, 1.0, 1), (9.0, 0.0, 0)] {
            let mut m = world();
            clock(&mut m, t);
            put(&mut m, 0x178, 1.0);
            put(&mut m, 0x17c, 5.0);
            m.write(AT + 0x170, &[1]).unwrap();
            m.set_u32(AT + 0x160, 0x60).unwrap();
            run(&mut m, &mut cpu(0x027f)).unwrap();
            assert_eq!(
                (
                    get(&m, 0xa4),
                    m.u8(AT + 0xa0).unwrap(),
                    m.u8(AT + 0x170).unwrap()
                ),
                (fade, flag, 1 - flag),
                "clock {t}"
            );
            assert_eq!(m.u32(AT + 0x90).unwrap(), 0x60);
        }
    }

    #[test]
    fn sse_matches_x86_on_nans_and_flags() {
        let mut s = Sse { mxcsr: 0x1f80 };
        assert_eq!(
            s.add(0x7f80_0001, 0x7fc0_1234),
            Ok(0x7fc0_0001),
            "first NaN, quieted"
        );
        assert_eq!(s.mxcsr & IE, IE);
        let mut s = Sse { mxcsr: 0x1f80 };
        assert_eq!(s.sub(F(f32::INFINITY), F(f32::INFINITY)), Ok(DEFAULT_NAN));
        assert_eq!(s.mul(F(3.0), F(1.0 / 3.0)).map(f32::from_bits), Ok(1.0));
        assert_eq!(s.mxcsr & PE, PE);
        let mut s = Sse { mxcsr: 0x1f80 };
        assert_eq!(s.add(F(1.0), F(2.0)), Ok(F(3.0)));
        assert_eq!(s.mxcsr, 0x1f80);
        assert_eq!(s.equal(F(0.0), F(-0.0)), Ok(true));
        assert_eq!(s.equal(0x7fc0_0000, 0x7fc0_0000), Ok(false));
        assert_eq!(s.mxcsr & IE, 0, "ucomiss is quiet on a QNaN");
    }
}
