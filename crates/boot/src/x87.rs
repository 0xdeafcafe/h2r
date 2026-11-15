//! The x87 for the boot fallback: one instruction at a time on the
//! interpreter's CPU, until crates/lift lowers x87 (docs/FLOAT.md).
//!
//! Values live in the canonical ext80 registers; every arithmetic,
//! conversion and transcendental goes through crates/x87 (exact software
//! ext80 under the control word's precision and rounding). Exceptions take
//! the masked response, which is what the game's control word asks for;
//! their sticky bits are recorded in the status word.

use iced_x86::{Instruction, MemorySize, Mnemonic, OpKind, Register};
use ir::interp::Cpu;
use x87::{Cmp, Const, F80, Flags, Mode};

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
    fn getv(&self, i: u16) -> F80 {
        F80::from_bytes(self.get(i))
    }
    fn setv(&mut self, i: u16, v: F80) {
        self.set(i, v.to_bytes())
    }
    fn mode(&self) -> Mode {
        Mode::from_cw(self.cpu.x87.control)
    }
    /// Record the masked response: sticky exception bits, and C1 as the
    /// operation reports it (round-up / stack fault direction).
    fn flags(&mut self, fl: Flags) {
        let st = &mut self.cpu.x87.status;
        *st |= fl.exceptions();
        *st = (*st & !C1) | if fl.contains(Flags::C1) { C1 } else { 0 };
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
    fn compare(&mut self, a: F80, b: F80, quiet: bool) {
        let (r, fl) = x87::compare(a, b, quiet);
        self.cpu.x87.status |= fl.exceptions();
        let c = match r {
            Cmp::Unordered => C0 | C2 | C3,
            Cmp::Greater => 0,
            Cmp::Less => C0,
            Cmp::Equal => C3,
        };
        self.cc(c);
    }
    fn comi(&mut self, a: F80, b: F80, quiet: bool) {
        let (r, fl) = x87::compare(a, b, quiet);
        self.cpu.x87.status |= fl.exceptions();
        // ZF 6, PF 2, CF 0; OF, SF, AF cleared.
        let (zf, pf, cf) = match r {
            Cmp::Unordered => (1, 1, 1),
            Cmp::Greater => (0, 0, 0),
            Cmp::Less => (0, 0, 1),
            Cmp::Equal => (1, 0, 0),
        };
        let mask: u32 = (1 << 0) | (1 << 2) | (1 << 4) | (1 << 6) | (1 << 7) | (1 << 11);
        self.cpu.eflags = (self.cpu.eflags & !mask) | (zf << 6) | (pf << 2) | cf;
        self.cpu.flags_valid |= mask;
    }
    fn rc(&self) -> x87::Rc {
        self.mode().rc
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
        MemorySize::Float32 => F80::from_f32_bits(u32::from_le_bytes(b[..4].try_into().unwrap())).to_bytes(),
        MemorySize::Float64 => F80::from_f64_bits(u64::from_le_bytes(b[..8].try_into().unwrap())).to_bytes(),
        MemorySize::Float80 => b,
        MemorySize::Int16 => F80::from_i64(i16::from_le_bytes(b[..2].try_into().unwrap()) as i64).to_bytes(),
        MemorySize::Int32 => F80::from_i64(i32::from_le_bytes(b[..4].try_into().unwrap()) as i64).to_bytes(),
        _ => F80::from_i64(i64::from_le_bytes(b[..8].try_into().unwrap())).to_bytes(),
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
    let binop = |f: &mut Fpu, mem: &mut dyn Mem| -> Result<(u16, F80, F80), Error> {
        if has_mem {
            let v = F80::from_bytes(load_mem(mem, addr, msize)?);
            Ok((0, f.getv(0), v))
        } else if regs.len() == 2 {
            Ok((regs[0], f.getv(regs[0]), f.getv(regs[1])))
        } else {
            // faddp etc. with no operands: st(1) op= st(0)
            Ok((1, f.getv(1), f.getv(0)))
        }
    };
    // Apply a rounded binary operation to st(d).
    let arith = |f: &mut Fpu, d: u16, r: (F80, Flags)| {
        f.setv(d, r.0);
        f.flags(r.1);
    };
    match m {
        M::Fld | M::Fild => {
            let v = if has_mem { load_mem(mem, addr, msize)? } else { f.get(regs[0]) };
            f.push(v);
        }
        M::Fld1 | M::Fldz | M::Fldpi | M::Fldl2e | M::Fldl2t | M::Fldlg2 | M::Fldln2 => {
            let c = match m {
                M::Fld1 => Const::One,
                M::Fldz => Const::Z,
                M::Fldpi => Const::Pi,
                M::Fldl2e => Const::L2E,
                M::Fldl2t => Const::L2T,
                M::Fldlg2 => Const::Lg2,
                _ => Const::Ln2,
            };
            let rc = f.rc();
            f.push(F80::constant(c, rc).to_bytes());
        }
        M::Fst | M::Fstp => {
            let v = f.get(0);
            if has_mem {
                match msize {
                    MemorySize::Float32 => {
                        let (b, fl) = F80::from_bytes(v).to_f32_bits(f.mode());
                        f.flags(fl);
                        mem.write(addr, &b.to_le_bytes()).map_err(Error::Memory)?
                    }
                    MemorySize::Float64 => {
                        let (b, fl) = F80::from_bytes(v).to_f64_bits(f.mode());
                        f.flags(fl);
                        mem.write(addr, &b.to_le_bytes()).map_err(Error::Memory)?
                    }
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
            let trunc = m == M::Fisttp;
            let (v, mode) = (f.getv(0), f.mode());
            match msize {
                MemorySize::Int16 => {
                    let (i, fl) = v.to_i16(mode, trunc);
                    f.flags(fl);
                    mem.write(addr, &i.to_le_bytes()).map_err(Error::Memory)?
                }
                MemorySize::Int32 => {
                    let (i, fl) = v.to_i32(mode, trunc);
                    f.flags(fl);
                    mem.write(addr, &i.to_le_bytes()).map_err(Error::Memory)?
                }
                MemorySize::Int64 => {
                    let (i, fl) = v.to_i64(mode, trunc);
                    f.flags(fl);
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
            let r = x87::add(a, b, f.mode());
            arith(&mut f, d, r);
            if m == M::Faddp {
                f.pop();
            }
        }
        M::Fsub | M::Fsubp | M::Fisub => {
            let (d, a, b) = binop(&mut f, mem)?;
            let r = x87::sub(a, b, f.mode());
            arith(&mut f, d, r);
            if m == M::Fsubp {
                f.pop();
            }
        }
        M::Fsubr | M::Fsubrp | M::Fisubr => {
            let (d, a, b) = binop(&mut f, mem)?;
            let r = x87::sub(b, a, f.mode());
            arith(&mut f, d, r);
            if m == M::Fsubrp {
                f.pop();
            }
        }
        M::Fmul | M::Fmulp | M::Fimul => {
            let (d, a, b) = binop(&mut f, mem)?;
            let r = x87::mul(a, b, f.mode());
            arith(&mut f, d, r);
            if m == M::Fmulp {
                f.pop();
            }
        }
        M::Fdiv | M::Fdivp | M::Fidiv => {
            let (d, a, b) = binop(&mut f, mem)?;
            let r = x87::div(a, b, f.mode());
            arith(&mut f, d, r);
            if m == M::Fdivp {
                f.pop();
            }
        }
        M::Fdivr | M::Fdivrp | M::Fidivr => {
            let (d, a, b) = binop(&mut f, mem)?;
            let r = x87::div(b, a, f.mode());
            arith(&mut f, d, r);
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
            let r = x87::sqrt(f.getv(0), f.mode());
            arith(&mut f, 0, r);
        }
        M::Fsin | M::Fcos | M::Fptan => {
            let v = f.getv(0);
            let t = match m {
                M::Fsin => x87::sin(v),
                M::Fcos => x87::cos(v),
                _ => x87::tan(v),
            };
            if t.out_of_range {
                f.cc(C2);
            } else {
                f.setv(0, t.value);
                f.flags(t.flags);
                f.cpu.x87.status &= !C2;
                if m == M::Fptan {
                    let rc = f.rc();
                    f.push(F80::constant(Const::One, rc).to_bytes());
                }
            }
        }
        M::Fsincos => {
            let (s, c) = x87::sincos(f.getv(0));
            if s.out_of_range {
                f.cc(C2);
            } else {
                f.setv(0, s.value);
                f.push(c.to_bytes());
                f.flags(s.flags);
                f.cpu.x87.status &= !C2;
            }
        }
        M::Fpatan | M::Fyl2x | M::Fyl2xp1 => {
            let (x, y) = (f.getv(0), f.getv(1));
            let r = match m {
                M::Fpatan => x87::patan(y, x),
                M::Fyl2x => x87::yl2x(y, x),
                _ => x87::yl2xp1(y, x),
            };
            arith(&mut f, 1, r);
            f.pop();
        }
        M::F2xm1 => {
            let r = x87::x2m1(f.getv(0));
            arith(&mut f, 0, r);
        }
        M::Fscale => {
            let rc = f.rc();
            let r = x87::scale(f.getv(0), f.getv(1), rc);
            arith(&mut f, 0, r);
        }
        M::Frndint => {
            let rc = f.rc();
            let r = x87::rndint(f.getv(0), rc);
            arith(&mut f, 0, r);
        }
        M::Fprem | M::Fprem1 => {
            let (x, y) = (f.getv(0), f.getv(1));
            let p = if m == M::Fprem { x87::prem(x, y) } else { x87::prem1(x, y) };
            f.setv(0, p.value);
            f.cpu.x87.status |= p.flags.exceptions();
            let mut c = if p.incomplete { C2 } else { 0 };
            if p.q & 1 != 0 {
                c |= C1;
            }
            if p.q & 2 != 0 {
                c |= C3;
            }
            if p.q & 4 != 0 {
                c |= C0;
            }
            f.cc(c);
        }
        M::Fxtract => {
            let (e, sig, fl) = x87::xtract(f.getv(0));
            f.cpu.x87.status |= fl.exceptions();
            f.setv(0, e);
            f.push(sig.to_bytes());
        }
        M::Fcom | M::Fcomp | M::Fcompp | M::Fucom | M::Fucomp | M::Fucompp | M::Ficom | M::Ficomp => {
            let b = if has_mem {
                F80::from_bytes(load_mem(mem, addr, msize)?)
            } else if let Some(&r) = regs.iter().find(|&&r| r != 0).or(regs.get(1)) {
                f.getv(r)
            } else {
                f.getv(1)
            };
            let a = f.getv(0);
            f.compare(a, b, matches!(m, M::Fucom | M::Fucomp | M::Fucompp));
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
            let b = f.getv(*regs.get(1).unwrap_or(&1));
            let a = f.getv(0);
            f.comi(a, b, matches!(m, M::Fucomi | M::Fucomip));
            if matches!(m, M::Fcomip | M::Fucomip) {
                f.pop();
            }
        }
        M::Ftst => {
            let a = f.getv(0);
            f.compare(a, F80::ZERO, false);
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
    use iced_x86::{Decoder, DecoderOptions};

    struct NoMem;
    impl Mem for NoMem {
        fn read(&mut self, addr: u32, _: &mut [u8]) -> Result<(), u32> {
            Err(addr)
        }
        fn write(&mut self, addr: u32, _: &[u8]) -> Result<(), u32> {
            Err(addr)
        }
    }

    fn run(cpu: &mut Cpu, code: &[u8]) {
        let mut d = Decoder::with_ip(32, code, 0x1000, DecoderOptions::NONE);
        while d.can_decode() {
            let ins = d.decode();
            assert!(exec(cpu, &mut NoMem, &ins).is_ok(), "{:?}", ins.mnemonic());
        }
    }

    #[test]
    fn arithmetic_goes_through_the_soft_float() {
        // fld1; fadd st(0),st(0); fsqrt  -> sqrt(2) rounded at PC=64
        let mut cpu = Cpu::default();
        run(&mut cpu, &[0xd9, 0xe8, 0xd8, 0xc0, 0xd9, 0xfa]);
        let f = Fpu { cpu: &mut cpu };
        let two = x87::add(F80::ONE, F80::ONE, f.mode()).0;
        assert_eq!(f.getv(0), x87::sqrt(two, f.mode()).0);
    }

    #[test]
    fn constants_follow_the_rounding_control() {
        let mut cpu = Cpu::default();
        run(&mut cpu, &[0xd9, 0xeb]); // fldpi
        let f = Fpu { cpu: &mut cpu };
        assert_eq!(f.getv(0), F80::constant(Const::Pi, x87::Rc::Near));
    }
}
