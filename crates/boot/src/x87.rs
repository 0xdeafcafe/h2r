//! A stand-in x87 for the boot fallback: one instruction at a time on the
//! interpreter's CPU, until crates/lift lowers x87 (docs/FLOAT.md).
//!
//! Values live in the canonical ext80 registers. Moves (`fld`/`fstp` of
//! registers and m80, `fxch`, `fild`, `fistp`) are exact on ext80;
//! arithmetic converts to `f64`, which docs/FLOAT.md shows gives the same
//! bits as the hardware at the PC=53 / round-to-nearest the game pins,
//! outside exponent-range corners. Transcendentals use host libm (not
//! bit-exact). No x87 exception is raised or flagged; all are treated as
//! masked, which is what the game's control word asks for.

use iced_x86::{Instruction, MemorySize, Mnemonic, OpKind, Register};
use ir::interp::Cpu;

/// A memory access the x87 instruction needs.
pub trait Mem {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), u32>;
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), u32>;
}

pub enum Error {
    /// A memory fault at the address.
    Memory(u32),
    Unsupported(String),
}

const C0: u16 = 1 << 8;
const C1: u16 = 1 << 9;
const C2: u16 = 1 << 10;
const C3: u16 = 1 << 14;

pub type Ext = [u8; 10];

fn ext_parts(e: &Ext) -> (bool, u16, u64) {
    let sig = u64::from_le_bytes(e[..8].try_into().unwrap());
    let se = u16::from_le_bytes([e[8], e[9]]);
    (se & 0x8000 != 0, se & 0x7fff, sig)
}

fn ext_make(sign: bool, exp: u16, sig: u64) -> Ext {
    let mut e = [0u8; 10];
    e[..8].copy_from_slice(&sig.to_le_bytes());
    e[8..].copy_from_slice(&((exp & 0x7fff) | if sign { 0x8000 } else { 0 }).to_le_bytes());
    e
}

pub fn f64_to_ext(v: f64) -> Ext {
    let b = v.to_bits();
    let sign = b >> 63 != 0;
    let exp = ((b >> 52) & 0x7ff) as i32;
    let mant = b & ((1 << 52) - 1);
    if exp == 0 && mant == 0 {
        return ext_make(sign, 0, 0);
    }
    if exp == 0x7ff {
        return ext_make(sign, 0x7fff, (1 << 63) | (mant << 11));
    }
    if exp == 0 {
        let lz = mant.leading_zeros() as i32; // >= 12
        let sig = mant << lz;
        // value = mant * 2^-1074 = sig * 2^(-1074 - lz); ext = sig * 2^(e - 16446)
        let e = 16446 - 1074 - lz;
        return ext_make(sign, e as u16, sig);
    }
    ext_make(sign, (exp - 1023 + 16383) as u16, (1 << 63) | (mant << 11))
}

fn scale2(mut v: f64, mut e: i32) -> f64 {
    while e > 1000 {
        v *= f64::from_bits(((1000 + 1023) as u64) << 52);
        e -= 1000;
    }
    while e < -1000 {
        v *= f64::from_bits(((-1000 + 1023) as u64) << 52);
        e += 1000;
    }
    v * f64::from_bits(((e + 1023) as u64) << 52)
}

pub fn ext_to_f64(x: &Ext) -> f64 {
    let (sign, exp, sig) = ext_parts(x);
    let s = if sign { -1.0 } else { 1.0 };
    if exp == 0 && sig == 0 {
        return if sign { -0.0 } else { 0.0 };
    }
    if exp == 0x7fff {
        if sig << 1 == 0 {
            return s * f64::INFINITY;
        }
        let payload = (sig >> 11) & ((1 << 52) - 1);
        return f64::from_bits(((sign as u64) << 63) | (0x7ff << 52) | payload | (1 << 51));
    }
    // sig as f64 rounds once to 53 bits; the power-of-two scale is exact
    // outside the subnormal/overflow range.
    s * scale2(sig as f64, exp as i32 - 16446)
}

fn int_to_ext(v: i64) -> Ext {
    if v == 0 {
        return ext_make(false, 0, 0);
    }
    let a = v.unsigned_abs();
    let lz = a.leading_zeros();
    ext_make(v < 0, (16383 + 63 - lz) as u16, a << lz)
}

/// ext80 to integer with the control word's rounding; None if out of range.
fn ext_to_int(x: &Ext, rc: u16, bits: u32) -> Option<i64> {
    let (sign, exp, sig) = ext_parts(x);
    if exp == 0x7fff {
        return None;
    }
    if exp == 0 && sig == 0 {
        return Some(0);
    }
    let shift = exp as i32 - 16446; // value = sig * 2^shift
    let mag: u128 = if shift >= 0 {
        if shift > 64 {
            return None;
        }
        (sig as u128) << shift
    } else {
        let k = (-shift) as u32;
        let (q, rem, half, any) = if k >= 128 {
            (0u128, 0u128, false, sig != 0)
        } else {
            let full = sig as u128;
            let q = full >> k;
            let rem = full & ((1u128 << k) - 1);
            let half_v = 1u128 << (k - 1);
            (q, rem, rem >= half_v, rem != 0)
        };
        let up = match rc {
            0 => {
                if k >= 128 {
                    false
                } else {
                    let half_v = 1u128 << (k - 1);
                    rem > half_v || (rem == half_v && q & 1 == 1)
                }
            }
            1 => sign && any,  // toward -inf
            2 => !sign && any, // toward +inf
            _ => false,
        };
        let _ = half;
        q + up as u128
    };
    let limit = 1u128 << (bits - 1);
    if (!sign && mag >= limit) || (sign && mag > limit) {
        return None;
    }
    Some(if sign { -(mag as i128) as i64 } else { mag as i64 })
}

fn classify_tag(x: &Ext) -> u16 {
    let (_, exp, sig) = ext_parts(x);
    if exp == 0 && sig == 0 {
        1
    } else if exp == 0x7fff || exp == 0 || sig >> 63 == 0 {
        2
    } else {
        0
    }
}

struct Fpu<'a> {
    cpu: &'a mut Cpu,
}

impl Fpu<'_> {
    fn top(&self) -> u16 {
        (self.cpu.x87.status >> 11) & 7
    }
    fn set_top(&mut self, t: u16) {
        self.cpu.x87.status = (self.cpu.x87.status & !(7 << 11)) | ((t & 7) << 11);
    }
    fn phys(&self, i: u16) -> usize {
        ((self.top() + i) & 7) as usize
    }
    fn get(&self, i: u16) -> Ext {
        self.cpu.x87.st[self.phys(i)]
    }
    fn set_tag(&mut self, p: usize, t: u16) {
        self.cpu.x87.tag = (self.cpu.x87.tag & !(3 << (2 * p))) | (t << (2 * p));
    }
    fn set(&mut self, i: u16, v: Ext) {
        let p = self.phys(i);
        self.cpu.x87.st[p] = v;
        self.set_tag(p, classify_tag(&v));
    }
    fn getf(&self, i: u16) -> f64 {
        ext_to_f64(&self.get(i))
    }
    fn setf(&mut self, i: u16, v: f64) {
        self.set(i, f64_to_ext(v))
    }
    fn push(&mut self, v: Ext) {
        let t = self.top().wrapping_sub(1) & 7;
        self.set_top(t);
        self.set(0, v);
    }
    fn pop(&mut self) {
        let p = self.phys(0);
        self.set_tag(p, 3);
        let t = (self.top() + 1) & 7;
        self.set_top(t);
    }
    fn cc(&mut self, c: u16) {
        self.cpu.x87.status = (self.cpu.x87.status & !(C0 | C1 | C2 | C3)) | c;
    }
    fn compare(&mut self, a: f64, b: f64) {
        let c = if a.is_nan() || b.is_nan() {
            C0 | C2 | C3
        } else if a > b {
            0
        } else if a < b {
            C0
        } else {
            C3
        };
        self.cc(c);
    }
    fn comi(&mut self, a: f64, b: f64) {
        // ZF 6, PF 2, CF 0; OF, SF, AF cleared.
        let (zf, pf, cf) = if a.is_nan() || b.is_nan() {
            (1, 1, 1)
        } else if a > b {
            (0, 0, 0)
        } else if a < b {
            (0, 0, 1)
        } else {
            (1, 0, 0)
        };
        let mask: u32 = (1 << 0) | (1 << 2) | (1 << 4) | (1 << 6) | (1 << 7) | (1 << 11);
        self.cpu.eflags = (self.cpu.eflags & !mask) | (zf << 6) | (pf << 2) | cf;
        self.cpu.flags_valid |= mask;
    }
    fn rc(&self) -> u16 {
        (self.cpu.x87.control >> 10) & 3
    }
}

fn reg_value(cpu: &Cpu, r: Register) -> u32 {
    use Register::*;
    let i = match r {
        EAX => 0,
        ECX => 1,
        EDX => 2,
        EBX => 3,
        ESP => 4,
        EBP => 5,
        ESI => 6,
        EDI => 7,
        None => return 0,
        _ => return 0,
    };
    cpu.gpr[i]
}

fn ea(cpu: &Cpu, ins: &Instruction) -> u32 {
    let mut a = reg_value(cpu, ins.memory_base())
        .wrapping_add(reg_value(cpu, ins.memory_index()).wrapping_mul(ins.memory_index_scale()))
        .wrapping_add(ins.memory_displacement32());
    if ins.memory_segment() == Register::FS {
        a = a.wrapping_add(cpu.fs_base);
    }
    a
}

fn st_index(r: Register) -> Option<u16> {
    let n = r as u32;
    let base = Register::ST0 as u32;
    (base..base + 8).contains(&n).then(|| (n - base) as u16)
}

fn load_mem(mem: &mut dyn Mem, addr: u32, size: MemorySize) -> Result<Ext, Error> {
    let mut b = [0u8; 10];
    let n = match size {
        MemorySize::Float32 | MemorySize::Int32 => 4,
        MemorySize::Float64 | MemorySize::Int64 => 8,
        MemorySize::Float80 => 10,
        MemorySize::Int16 => 2,
        s => return Err(Error::Unsupported(format!("x87 load size {s:?}"))),
    };
    mem.read(addr, &mut b[..n]).map_err(Error::Memory)?;
    Ok(match size {
        MemorySize::Float32 => f64_to_ext(f32::from_le_bytes(b[..4].try_into().unwrap()) as f64),
        MemorySize::Float64 => f64_to_ext(f64::from_le_bytes(b[..8].try_into().unwrap())),
        MemorySize::Float80 => b,
        MemorySize::Int16 => int_to_ext(i16::from_le_bytes(b[..2].try_into().unwrap()) as i64),
        MemorySize::Int32 => int_to_ext(i32::from_le_bytes(b[..4].try_into().unwrap()) as i64),
        _ => int_to_ext(i64::from_le_bytes(b[..8].try_into().unwrap())),
    })
}

/// Execute one x87 instruction. Returns the next EIP.
pub fn exec(cpu: &mut Cpu, mem: &mut dyn Mem, ins: &Instruction) -> Result<u32, Error> {
    use Mnemonic as M;
    let next = ins.next_ip32();
    let m = ins.mnemonic();
    let has_mem = (0..ins.op_count()).any(|i| ins.op_kind(i) == OpKind::Memory);
    let addr = if has_mem { ea(cpu, ins) } else { 0 };
    let msize = ins.memory_size();
    let regs: Vec<u16> = (0..ins.op_count()).filter_map(|i| st_index(ins.op_register(i))).collect();
    let mut f = Fpu { cpu };

    // (dst index, src value) for the binary arithmetic forms.
    let binop = |f: &mut Fpu, mem: &mut dyn Mem| -> Result<(u16, f64, f64), Error> {
        if has_mem {
            let v = ext_to_f64(&load_mem(mem, addr, msize)?);
            Ok((0, f.getf(0), v))
        } else if regs.len() == 2 {
            Ok((regs[0], f.getf(regs[0]), f.getf(regs[1])))
        } else {
            // faddp etc. with no operands: st(1) op= st(0)
            Ok((1, f.getf(1), f.getf(0)))
        }
    };
    match m {
        M::Fld | M::Fild => {
            let v = if has_mem { load_mem(mem, addr, msize)? } else { f.get(regs[0]) };
            f.push(v);
        }
        M::Fld1 => f.push(f64_to_ext(1.0)),
        M::Fldz => f.push(f64_to_ext(0.0)),
        M::Fldpi => f.push(f64_to_ext(std::f64::consts::PI)),
        M::Fldl2e => f.push(f64_to_ext(std::f64::consts::LOG2_E)),
        M::Fldl2t => f.push(f64_to_ext(std::f64::consts::LOG2_10)),
        M::Fldlg2 => f.push(f64_to_ext(std::f64::consts::LOG10_2)),
        M::Fldln2 => f.push(f64_to_ext(std::f64::consts::LN_2)),
        M::Fst | M::Fstp => {
            let v = f.get(0);
            if has_mem {
                match msize {
                    MemorySize::Float32 => {
                        mem.write(addr, &(ext_to_f64(&v) as f32).to_le_bytes()).map_err(Error::Memory)?
                    }
                    MemorySize::Float64 => mem.write(addr, &ext_to_f64(&v).to_le_bytes()).map_err(Error::Memory)?,
                    MemorySize::Float80 => mem.write(addr, &v).map_err(Error::Memory)?,
                    s => return Err(Error::Unsupported(format!("fst size {s:?}"))),
                }
            } else {
                f.set(regs[0], v);
            }
            if m == M::Fstp {
                f.pop();
            }
        }
        M::Fist | M::Fistp | M::Fisttp => {
            let rc = if m == M::Fisttp { 3 } else { f.rc() };
            let v = f.get(0);
            match msize {
                MemorySize::Int16 => {
                    let i = ext_to_int(&v, rc, 16).unwrap_or(-0x8000) as i16;
                    mem.write(addr, &i.to_le_bytes()).map_err(Error::Memory)?
                }
                MemorySize::Int32 => {
                    let i = ext_to_int(&v, rc, 32).unwrap_or(i32::MIN as i64) as i32;
                    mem.write(addr, &i.to_le_bytes()).map_err(Error::Memory)?
                }
                MemorySize::Int64 => {
                    let i = ext_to_int(&v, rc, 64).unwrap_or(i64::MIN);
                    mem.write(addr, &i.to_le_bytes()).map_err(Error::Memory)?
                }
                s => return Err(Error::Unsupported(format!("fist size {s:?}"))),
            }
            if m != M::Fist {
                f.pop();
            }
        }
        M::Fadd | M::Faddp | M::Fiadd => {
            let (d, a, b) = binop(&mut f, mem)?;
            f.setf(d, a + b);
            if m == M::Faddp {
                f.pop();
            }
        }
        M::Fsub | M::Fsubp | M::Fisub => {
            let (d, a, b) = binop(&mut f, mem)?;
            f.setf(d, a - b);
            if m == M::Fsubp {
                f.pop();
            }
        }
        M::Fsubr | M::Fsubrp | M::Fisubr => {
            let (d, a, b) = binop(&mut f, mem)?;
            f.setf(d, b - a);
            if m == M::Fsubrp {
                f.pop();
            }
        }
        M::Fmul | M::Fmulp | M::Fimul => {
            let (d, a, b) = binop(&mut f, mem)?;
            f.setf(d, a * b);
            if m == M::Fmulp {
                f.pop();
            }
        }
        M::Fdiv | M::Fdivp | M::Fidiv => {
            let (d, a, b) = binop(&mut f, mem)?;
            f.setf(d, a / b);
            if m == M::Fdivp {
                f.pop();
            }
        }
        M::Fdivr | M::Fdivrp | M::Fidivr => {
            let (d, a, b) = binop(&mut f, mem)?;
            f.setf(d, b / a);
            if m == M::Fdivrp {
                f.pop();
            }
        }
        M::Fchs => {
            let mut v = f.get(0);
            v[9] ^= 0x80;
            f.set(0, v);
        }
        M::Fabs => {
            let mut v = f.get(0);
            v[9] &= 0x7f;
            f.set(0, v);
        }
        M::Fsqrt => {
            let v = f.getf(0);
            f.setf(0, v.sqrt());
        }
        M::Fsin => {
            let v = f.getf(0);
            f.setf(0, v.sin());
            f.cc(0);
        }
        M::Fcos => {
            let v = f.getf(0);
            f.setf(0, v.cos());
            f.cc(0);
        }
        M::Fsincos => {
            let v = f.getf(0);
            f.setf(0, v.sin());
            f.push(f64_to_ext(v.cos()));
            f.cc(0);
        }
        M::Fptan => {
            let v = f.getf(0);
            f.setf(0, v.tan());
            f.push(f64_to_ext(1.0));
            f.cc(0);
        }
        M::Fpatan => {
            let (x, y) = (f.getf(0), f.getf(1));
            f.setf(1, y.atan2(x));
            f.pop();
        }
        M::F2xm1 => {
            let v = f.getf(0);
            f.setf(0, v.exp2() - 1.0);
        }
        M::Fyl2x => {
            let (x, y) = (f.getf(0), f.getf(1));
            f.setf(1, y * x.log2());
            f.pop();
        }
        M::Fyl2xp1 => {
            let (x, y) = (f.getf(0), f.getf(1));
            f.setf(1, y * (x.ln_1p() / std::f64::consts::LN_2));
            f.pop();
        }
        M::Fscale => {
            let (x, s) = (f.getf(0), f.getf(1).trunc());
            let e = s.clamp(-4000.0, 4000.0) as i32;
            f.setf(0, scale2(x, e));
        }
        M::Frndint => {
            let v = f.getf(0);
            let r = match f.rc() {
                0 => v.round_ties_even(),
                1 => v.floor(),
                2 => v.ceil(),
                _ => v.trunc(),
            };
            f.setf(0, r);
        }
        M::Fprem | M::Fprem1 => {
            let (x, y) = (f.getf(0), f.getf(1));
            let q = if m == M::Fprem { (x / y).trunc() } else { (x / y).round_ties_even() };
            let r = if m == M::Fprem { x % y } else { x - q * y };
            let qi = q.abs() as u64;
            f.setf(0, r);
            let mut c = 0;
            if qi & 1 != 0 {
                c |= C1;
            }
            if qi & 2 != 0 {
                c |= C3;
            }
            if qi & 4 != 0 {
                c |= C0;
            }
            f.cc(c);
        }
        M::Fxtract => {
            let v = f.get(0);
            let (sign, exp, sig) = ext_parts(&v);
            f.setf(0, exp as f64 - 16383.0);
            f.push(ext_make(sign, 16383, sig));
        }
        M::Fcom | M::Fcomp | M::Fcompp | M::Fucom | M::Fucomp | M::Fucompp | M::Ficom | M::Ficomp => {
            let b = if has_mem {
                ext_to_f64(&load_mem(mem, addr, msize)?)
            } else if let Some(&r) = regs.iter().find(|&&r| r != 0).or(regs.get(1)) {
                f.getf(r)
            } else {
                f.getf(1)
            };
            let a = f.getf(0);
            f.compare(a, b);
            match m {
                M::Fcomp | M::Fucomp | M::Ficomp => f.pop(),
                M::Fcompp | M::Fucompp => {
                    f.pop();
                    f.pop();
                }
                _ => {}
            }
        }
        M::Fcomi | M::Fcomip | M::Fucomi | M::Fucomip => {
            let b = f.getf(*regs.get(1).unwrap_or(&1));
            let a = f.getf(0);
            f.comi(a, b);
            if matches!(m, M::Fcomip | M::Fucomip) {
                f.pop();
            }
        }
        M::Ftst => {
            let a = f.getf(0);
            f.compare(a, 0.0);
        }
        M::Fxam => {
            let p = f.phys(0);
            let empty = (f.cpu.x87.tag >> (2 * p)) & 3 == 3;
            let v = f.get(0);
            let (sign, exp, sig) = ext_parts(&v);
            let class = if empty {
                C3 | C0
            } else if exp == 0x7fff {
                if sig << 1 == 0 { C2 | C0 } else { C0 }
            } else if exp == 0 && sig == 0 {
                C3
            } else if exp == 0 {
                C3 | C2
            } else {
                C2
            };
            f.cc(class | if sign { C1 } else { 0 });
        }
        M::Fxch => {
            let i = *regs.iter().find(|&&r| r != 0).unwrap_or(&1);
            let (a, b) = (f.get(0), f.get(i));
            f.set(0, b);
            f.set(i, a);
        }
        M::Fnstsw | M::Fstsw => {
            let s = f.cpu.x87.status;
            if has_mem {
                mem.write(addr, &s.to_le_bytes()).map_err(Error::Memory)?;
            } else {
                f.cpu.gpr[0] = (f.cpu.gpr[0] & 0xffff_0000) | s as u32;
            }
        }
        M::Fnstcw | M::Fstcw => mem.write(addr, &f.cpu.x87.control.to_le_bytes()).map_err(Error::Memory)?,
        M::Fldcw => {
            let mut b = [0u8; 2];
            mem.read(addr, &mut b).map_err(Error::Memory)?;
            f.cpu.x87.control = u16::from_le_bytes(b);
        }
        M::Fninit | M::Finit => {
            f.cpu.x87 = ir::interp::X87::default();
        }
        M::Fnclex | M::Fclex => f.cpu.x87.status &= 0x7f00,
        M::Ffree | M::Ffreep => {
            let p = f.phys(regs[0]);
            f.set_tag(p, 3);
            if m == M::Ffreep {
                f.pop();
            }
        }
        M::Fincstp => {
            let t = (f.top() + 1) & 7;
            f.set_top(t);
        }
        M::Fdecstp => {
            let t = f.top().wrapping_sub(1) & 7;
            f.set_top(t);
        }
        M::Wait | M::Fnop => {}
        M::Fcmovb | M::Fcmove | M::Fcmovbe | M::Fcmovu | M::Fcmovnb | M::Fcmovne | M::Fcmovnbe | M::Fcmovnu => {
            let fl = f.cpu.eflags;
            let (cf, zf, pf) = (fl & 1 != 0, fl & 0x40 != 0, fl & 4 != 0);
            let c = match m {
                M::Fcmovb => cf,
                M::Fcmove => zf,
                M::Fcmovbe => cf || zf,
                M::Fcmovu => pf,
                M::Fcmovnb => !cf,
                M::Fcmovne => !zf,
                M::Fcmovnbe => !(cf || zf),
                _ => !pf,
            };
            if c {
                let v = f.get(regs[1]);
                f.set(0, v);
            }
        }
        other => return Err(Error::Unsupported(format!("x87 {other:?}"))),
    }
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_roundtrips_doubles() {
        for v in [0.0, -0.0, 1.0, -2.5, 1e300, 1e-300, 5e-324, f64::INFINITY, 3.141592653589793] {
            assert_eq!(ext_to_f64(&f64_to_ext(v)).to_bits(), v.to_bits(), "{v}");
        }
    }

    #[test]
    fn ints_are_exact() {
        for v in [0i64, 1, -1, 1 << 62, i64::MIN, i64::MAX, 123456789012345] {
            assert_eq!(ext_to_int(&int_to_ext(v), 0, 64), Some(v));
        }
        assert_eq!(ext_to_int(&f64_to_ext(2.5), 0, 32), Some(2));
        assert_eq!(ext_to_int(&f64_to_ext(3.5), 0, 32), Some(4));
        assert_eq!(ext_to_int(&f64_to_ext(-2.5), 3, 32), Some(-2));
        assert_eq!(ext_to_int(&f64_to_ext(-2.5), 1, 32), Some(-3));
        assert_eq!(ext_to_int(&f64_to_ext(3e9), 0, 32), None);
    }
}
