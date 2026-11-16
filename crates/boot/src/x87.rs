//! The x87 for the boot fallback: one instruction at a time on the
//! interpreter's CPU. crates/lift lowers x87 to helper calls now
//! (docs/lifter/FLOAT.md), so generated code no longer needs this; it remains the
//! interpreter-side x87 for `crate::fallback`, which interprets whatever
//! the generated module stops on.
//!
//! This is a decoder over crates/x87's state machine (`x87::abi`, the same
//! helpers generated code will call): stack faults, source-format
//! exceptions, condition codes and the masked responses all come from
//! there, so the fallback cannot diverge from it. A helper reporting a
//! pending unmasked exception (#MF) or an unmodelled one stops the
//! fallback; the game masks everything.

use iced_x86::{Instruction, MemorySize, Mnemonic, OpKind, Register};
use ir::interp::Cpu;
use x87::abi::{self, X87Abi, arith_op, src_kind, status, unary_op};

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

fn to_abi(x: &ir::interp::X87) -> X87Abi {
    X87Abi {
        st: x.st,
        control: x.control,
        status: x.status,
        tag: x.tag,
        opcode: x.opcode,
        instruction_pointer: x.instruction_pointer,
        data_pointer: x.data_pointer,
        instruction_selector: x.instruction_selector,
        data_selector: x.data_selector,
    }
}

fn from_abi(s: &X87Abi) -> ir::interp::X87 {
    ir::interp::X87 {
        st: s.st,
        control: s.control,
        status: s.status,
        tag: s.tag,
        opcode: s.opcode,
        instruction_pointer: s.instruction_pointer,
        data_pointer: s.data_pointer,
        instruction_selector: s.instruction_selector,
        data_selector: s.data_selector,
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

fn st_index(r: Register) -> Option<u32> {
    let n = r as u32;
    let base = Register::ST0 as u32;
    (base..base + 8).contains(&n).then(|| n - base)
}

fn read<const N: usize>(mem: &mut dyn Mem, addr: u32) -> Result<[u8; N], Error> {
    let mut b = [0u8; N];
    mem.read(addr, &mut b).map_err(Error::Memory)?;
    Ok(b)
}

/// A memory operand as an `arith`/`cmp` source.
fn mem_source(mem: &mut dyn Mem, addr: u32, size: MemorySize) -> Result<(u32, u64), Error> {
    Ok(match size {
        MemorySize::Float32 => (src_kind::F32, u32::from_le_bytes(read(mem, addr)?) as u64),
        MemorySize::Float64 => (src_kind::F64, u64::from_le_bytes(read(mem, addr)?)),
        MemorySize::Int16 => (src_kind::I16, u16::from_le_bytes(read(mem, addr)?) as u64),
        MemorySize::Int32 => (src_kind::I32, u32::from_le_bytes(read(mem, addr)?) as u64),
        s => return Err(Error::Unsupported(format!("x87 source size {s:?}"))),
    })
}

/// A helper's status: OK goes on, the rest stop the fallback.
fn check(r: u32, m: Mnemonic) -> Result<u32, Error> {
    match r & status::MASK {
        status::OK | status::SUPPRESS => Ok(r & !status::MASK),
        status::MF => Err(Error::Unsupported(format!("x87 {m:?}: unmasked exception pending (#MF)"))),
        _ => Err(Error::Unsupported(format!("x87 {m:?}: unmasked OE/UE/PE or directed-RC transcendental"))),
    }
}

/// Phase 2 and 3 of a store: write `bytes` on OK, then commit the status
/// word (and pop) only if the write succeeded. SUPPRESS writes nothing.
fn store(s: &mut X87Abi, mem: &mut dyn Mem, addr: u32, m: Mnemonic, (code, sw): (u32, u16), bytes: &[u8], pop: bool) -> Result<(), Error> {
    match code {
        status::OK => {
            mem.write(addr, bytes).map_err(Error::Memory)?;
            abi::store_commit(s, sw, pop);
        }
        status::SUPPRESS => abi::store_commit(s, sw, false),
        c => {
            check(c, m)?;
        }
    }
    Ok(())
}

/// Execute one x87 instruction. Returns the next EIP.
pub fn exec(cpu: &mut Cpu, mem: &mut dyn Mem, ins: &Instruction) -> Result<u32, Error> {
    let mut s = to_abi(&cpu.x87);
    exec_on(&mut s, cpu, mem, ins)?;
    cpu.x87 = from_abi(&s);
    Ok(ins.next_ip32())
}

fn exec_on(s: &mut X87Abi, cpu: &mut Cpu, mem: &mut dyn Mem, ins: &Instruction) -> Result<(), Error> {
    use Mnemonic as M;
    let m = ins.mnemonic();
    let has_mem = (0..ins.op_count()).any(|i| ins.op_kind(i) == OpKind::Memory);
    let addr = if has_mem { ea(cpu, ins) } else { 0 };
    let msize = ins.memory_size();
    let regs: Vec<u32> = (0..ins.op_count()).filter_map(|i| st_index(ins.op_register(i))).collect();
    // The ST(i) operand of a one-register form, decoded as [ST0, STi] or
    // [STi]; ST(1) when the form has none. An explicit ST(0) stays 0.
    let sti = match regs.as_slice() {
        [_, i] => *i,
        [i] => *i,
        _ => 1,
    };
    let ck = |r: u32| check(r, m);
    match m {
        M::Fld if has_mem => {
            let r = match msize {
                MemorySize::Float32 => abi::ld_f32(s, u32::from_le_bytes(read(mem, addr)?)),
                MemorySize::Float64 => abi::ld_f64(s, u64::from_le_bytes(read(mem, addr)?)),
                MemorySize::Float80 => abi::ld_f80(s, read(mem, addr)?),
                z => return Err(Error::Unsupported(format!("fld size {z:?}"))),
            };
            ck(r)?;
        }
        M::Fld => {
            ck(abi::ld_st(s, sti))?;
        }
        M::Fild => {
            let v = match msize {
                MemorySize::Int16 => i16::from_le_bytes(read(mem, addr)?) as i64,
                MemorySize::Int32 => i32::from_le_bytes(read(mem, addr)?) as i64,
                MemorySize::Int64 => i64::from_le_bytes(read(mem, addr)?),
                z => return Err(Error::Unsupported(format!("fild size {z:?}"))),
            };
            ck(abi::ld_int(s, v))?;
        }
        M::Fbld => {
            ck(abi::ld_bcd(s, read(mem, addr)?))?;
        }
        M::Fld1 | M::Fldz | M::Fldpi | M::Fldl2e | M::Fldl2t | M::Fldlg2 | M::Fldln2 => {
            let which = match m {
                M::Fld1 => 0,
                M::Fldz => 1,
                M::Fldpi => 2,
                M::Fldl2e => 3,
                M::Fldl2t => 4,
                M::Fldlg2 => 5,
                _ => 6,
            };
            ck(abi::ld_const(s, which))?;
        }
        M::Fst | M::Fstp if has_mem => {
            let pop = m == M::Fstp;
            match msize {
                MemorySize::Float32 => {
                    let (c, st) = abi::st_f32(s, 0);
                    store(s, mem, addr, m, (c, st.status), &st.value.to_le_bytes(), pop)?;
                }
                MemorySize::Float64 => {
                    let (c, st) = abi::st_f64(s, 0);
                    store(s, mem, addr, m, (c, st.status), &st.value.to_le_bytes(), pop)?;
                }
                MemorySize::Float80 => {
                    let (c, st) = abi::st_f80(s, 0);
                    store(s, mem, addr, m, (c, st.status), &st.value, pop)?;
                }
                z => return Err(Error::Unsupported(format!("fst size {z:?}"))),
            }
        }
        M::Fst | M::Fstp => {
            ck(abi::st_st(s, sti, m == M::Fstp))?;
        }
        M::Fist | M::Fistp => {
            let pop = m == M::Fistp;
            match msize {
                MemorySize::Int16 => {
                    let (c, st) = abi::st_i16(s, 0);
                    store(s, mem, addr, m, (c, st.status), &st.value.to_le_bytes(), pop)?;
                }
                MemorySize::Int32 => {
                    let (c, st) = abi::st_i32(s, 0);
                    store(s, mem, addr, m, (c, st.status), &st.value.to_le_bytes(), pop)?;
                }
                MemorySize::Int64 => {
                    let (c, st) = abi::st_i64(s, 0);
                    store(s, mem, addr, m, (c, st.status), &st.value.to_le_bytes(), pop)?;
                }
                z => return Err(Error::Unsupported(format!("fist size {z:?}"))),
            }
        }
        M::Fbstp => {
            let (c, st) = abi::st_bcd(s, 0);
            store(s, mem, addr, m, (c, st.status), &st.value, true)?;
        }
        M::Fadd | M::Faddp | M::Fiadd | M::Fsub | M::Fsubp | M::Fisub | M::Fsubr | M::Fsubrp | M::Fisubr | M::Fmul | M::Fmulp | M::Fimul | M::Fdiv | M::Fdivp | M::Fidiv | M::Fdivr | M::Fdivrp | M::Fidivr => {
            let (op, pop) = match m {
                M::Fadd | M::Fiadd => (arith_op::ADD, false),
                M::Faddp => (arith_op::ADD, true),
                M::Fsub | M::Fisub => (arith_op::SUB, false),
                M::Fsubp => (arith_op::SUB, true),
                M::Fsubr | M::Fisubr => (arith_op::SUBR, false),
                M::Fsubrp => (arith_op::SUBR, true),
                M::Fmul | M::Fimul => (arith_op::MUL, false),
                M::Fmulp => (arith_op::MUL, true),
                M::Fdiv | M::Fidiv => (arith_op::DIV, false),
                M::Fdivp => (arith_op::DIV, true),
                M::Fdivr | M::Fidivr => (arith_op::DIVR, false),
                _ => (arith_op::DIVR, true),
            };
            let op = op | if pop { abi::ARITH_POP } else { 0 };
            // ST(dst) = ST(dst) op src.
            let (dst, kind, src) = if has_mem {
                let (k, v) = mem_source(mem, addr, msize)?;
                (0, k, v)
            } else if let [d, r] = regs.as_slice() {
                (*d, src_kind::ST, *r as u64)
            } else {
                (1, src_kind::ST, 0)
            };
            ck(abi::arith(s, op, dst, kind, src))?;
        }
        M::Fsqrt | M::Fabs | M::Fchs | M::Frndint | M::Fsin | M::Fcos | M::Fptan | M::Fpatan | M::Fyl2x | M::Fyl2xp1 | M::F2xm1 | M::Fscale | M::Fxtract | M::Fsincos | M::Fprem | M::Fprem1 => {
            let op = match m {
                M::Fsqrt => unary_op::SQRT,
                M::Fabs => unary_op::ABS,
                M::Fchs => unary_op::CHS,
                M::Frndint => unary_op::RNDINT,
                M::Fsin => unary_op::SIN,
                M::Fcos => unary_op::COS,
                M::Fptan => unary_op::TAN,
                M::Fpatan => unary_op::PATAN,
                M::Fyl2x => unary_op::YL2X,
                M::Fyl2xp1 => unary_op::YL2XP1,
                M::F2xm1 => unary_op::X2M1,
                M::Fscale => unary_op::SCALE,
                M::Fxtract => unary_op::XTRACT,
                M::Fsincos => unary_op::SINCOS,
                M::Fprem => unary_op::PREM,
                _ => unary_op::PREM1,
            };
            ck(abi::unary(s, op))?;
        }
        M::Fcom | M::Fcomp | M::Fcompp | M::Fucom | M::Fucomp | M::Fucompp | M::Ficom | M::Ficomp | M::Ftst => {
            let kind = match m {
                M::Fucom | M::Fucomp | M::Fucompp => 1,
                M::Ficom | M::Ficomp => 2,
                _ => 0,
            };
            let pop = match m {
                M::Fcomp | M::Fucomp | M::Ficomp => 1,
                M::Fcompp | M::Fucompp => 2,
                _ => 0,
            };
            let (skind, src) = if m == M::Ftst {
                (src_kind::F32, 0)
            } else if has_mem {
                mem_source(mem, addr, msize)?
            } else {
                (src_kind::ST, sti as u64)
            };
            // C3/C2/C0 are already in the status word.
            ck(abi::cmp(s, kind, skind, src, pop, false))?;
        }
        M::Fcomi | M::Fcomip | M::Fucomi | M::Fucomip => {
            let kind = if matches!(m, M::Fucomi | M::Fucomip) { 1 } else { 0 };
            let pop = matches!(m, M::Fcomip | M::Fucomip) as u32;
            let r = abi::cmp(s, kind, src_kind::ST, sti as u64, pop, true);
            let low = ck(r)?;
            // OF, SF, AF cleared; ZF/PF/CF from the compare unless it was
            // suppressed.
            let zpc = abi::EFL_ZF | abi::EFL_PF | abi::EFL_CF;
            let mut clear = (1 << 4) | (1 << 7) | (1 << 11);
            if r & status::MASK != status::SUPPRESS {
                clear |= zpc;
            }
            cpu.eflags = (cpu.eflags & !clear) | (low & zpc & clear);
            cpu.flags_valid |= zpc | (1 << 4) | (1 << 7) | (1 << 11);
        }
        M::Fxam => {
            ck(abi::xam(s))?;
        }
        M::Fxch => {
            ck(abi::xch(s, sti))?;
        }
        M::Fnstsw | M::Fstsw => {
            if m == M::Fstsw {
                ck(abi::wait(s))?;
            }
            if has_mem {
                mem.write(addr, &s.status.to_le_bytes()).map_err(Error::Memory)?;
            } else {
                cpu.gpr[0] = (cpu.gpr[0] & 0xffff_0000) | s.status as u32;
            }
        }
        M::Fnstcw | M::Fstcw => {
            if m == M::Fstcw {
                ck(abi::wait(s))?;
            }
            mem.write(addr, &s.control.to_le_bytes()).map_err(Error::Memory)?;
        }
        M::Fldcw => {
            ck(abi::ldcw(s, u16::from_le_bytes(read(mem, addr)?)))?;
        }
        M::Fninit | M::Finit => {
            if m == M::Finit {
                ck(abi::wait(s))?;
            }
            abi::init(s);
        }
        M::Fnclex | M::Fclex => {
            if m == M::Fclex {
                ck(abi::wait(s))?;
            }
            abi::clex(s);
        }
        M::Fnstenv | M::Fstenv | M::Fnsave | M::Fsave => {
            if matches!(m, M::Fstenv | M::Fsave) {
                ck(abi::wait(s))?;
            }
            let op = if matches!(m, M::Fnstenv | M::Fstenv) { abi::env_op::FNSTENV } else { abi::env_op::FNSAVE };
            let image = abi::save_image(s, op);
            mem.write(addr, &image[..abi::env_len(op)]).map_err(Error::Memory)?;
            abi::save_commit(s, op);
        }
        M::Fldenv | M::Frstor => {
            ck(abi::wait(s))?;
            let op = if m == M::Fldenv { abi::env_op::FLDENV } else { abi::env_op::FRSTOR };
            let mut image = [0u8; 108];
            mem.read(addr, &mut image[..abi::env_len(op)]).map_err(Error::Memory)?;
            ck(abi::load_env(s, op, &image))?;
        }
        M::Ffree | M::Ffreep => {
            ck(abi::free(s, sti, m == M::Ffreep))?;
        }
        M::Fincstp => {
            ck(abi::adj_top(s, 1))?;
        }
        M::Fdecstp => {
            ck(abi::adj_top(s, -1))?;
        }
        M::Wait | M::Fnop => {
            ck(abi::wait(s))?;
        }
        M::Fcmovb | M::Fcmove | M::Fcmovbe | M::Fcmovu | M::Fcmovnb | M::Fcmovne | M::Fcmovnbe | M::Fcmovnu => {
            let fl = cpu.eflags;
            let (cf, zf, pf) = (fl & 1 != 0, fl & 0x40 != 0, fl & 4 != 0);
            let take = match m {
                M::Fcmovb => cf,
                M::Fcmove => zf,
                M::Fcmovbe => cf || zf,
                M::Fcmovu => pf,
                M::Fcmovnb => !cf,
                M::Fcmovne => !zf,
                M::Fcmovnbe => !(cf || zf),
                _ => !pf,
            };
            ck(abi::cmov(s, sti, take))?;
        }
        other => return Err(Error::Unsupported(format!("x87 {other:?}"))),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced_x86::{Decoder, DecoderOptions};
    use x87::F80;

    /// 64 bytes of guest memory at 0x2000.
    struct Buf([u8; 64]);
    impl Mem for Buf {
        fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), u32> {
            let at = addr.checked_sub(0x2000).ok_or(addr)? as usize;
            buf.copy_from_slice(self.0.get(at..at + buf.len()).ok_or(addr)?);
            Ok(())
        }
        fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), u32> {
            let at = addr.checked_sub(0x2000).ok_or(addr)? as usize;
            self.0.get_mut(at..at + data.len()).ok_or(addr)?.copy_from_slice(data);
            Ok(())
        }
    }

    fn run_with(cpu: &mut Cpu, mem: &mut Buf, code: &[u8]) {
        let mut d = Decoder::with_ip(32, code, 0x1000, DecoderOptions::NONE);
        while d.can_decode() {
            let ins = d.decode();
            assert!(exec(cpu, mem, &ins).is_ok(), "{:?}", ins.mnemonic());
        }
    }

    fn run(cpu: &mut Cpu, code: &[u8]) {
        run_with(cpu, &mut Buf([0; 64]), code)
    }

    fn st(cpu: &Cpu, i: u32) -> Option<F80> {
        to_abi(&cpu.x87).st(i)
    }

    const IE: u16 = abi::H2_FP_IE;
    const DE: u16 = abi::H2_FP_DE;
    const SF: u16 = abi::H2_FP_SF;

    #[test]
    fn arithmetic_goes_through_the_soft_float() {
        // fld1; fadd st(0),st(0); fsqrt  -> sqrt(2) rounded at PC=64
        let mut cpu = Cpu::default();
        run(&mut cpu, &[0xd9, 0xe8, 0xd8, 0xc0, 0xd9, 0xfa]);
        let mode = x87::Mode::from_cw(cpu.x87.control);
        let two = x87::add(F80::ONE, F80::ONE, mode).0;
        assert_eq!(st(&cpu, 0), Some(x87::sqrt(two, mode).0));
        assert_eq!(cpu.x87.status & 0x7f & !abi::H2_FP_PE, 0);
    }

    #[test]
    fn constants_follow_the_rounding_control() {
        let mut cpu = Cpu::default();
        run(&mut cpu, &[0xd9, 0xeb]); // fldpi
        assert_eq!(st(&cpu, 0), Some(F80::constant(x87::Const::Pi, x87::Rc::Near)));
    }

    #[test]
    fn empty_stack_arithmetic_is_a_stack_fault() {
        // fninit; fadd st(0),st(0)
        let mut cpu = Cpu::default();
        run(&mut cpu, &[0xdb, 0xe3, 0xd8, 0xc0]);
        assert_eq!(cpu.x87.status & (IE | SF), IE | SF);
        assert_eq!(cpu.x87.status & abi::SW_C1, 0);
        assert_eq!(st(&cpu, 0), Some(F80::INDEFINITE));
    }

    #[test]
    fn stack_overflow_is_a_stack_fault() {
        // nine fld1
        let mut cpu = Cpu::default();
        run(&mut cpu, &[0xd9, 0xe8].repeat(9));
        assert_eq!(cpu.x87.status & (IE | SF), IE | SF);
        assert_ne!(cpu.x87.status & abi::SW_C1, 0, "C1 marks overflow");
        assert_eq!(st(&cpu, 0), Some(F80::INDEFINITE));
    }

    #[test]
    fn loads_raise_source_format_exceptions() {
        // fld dword [0x2000]: an f32 denormal sets DE.
        let fld = [0xd9, 0x05, 0x00, 0x20, 0x00, 0x00];
        let mut cpu = Cpu::default();
        let mut mem = Buf([0; 64]);
        mem.0[..4].copy_from_slice(&1u32.to_le_bytes());
        run_with(&mut cpu, &mut mem, &fld);
        assert_eq!(cpu.x87.status & (IE | DE), DE);

        // An SNaN raises IE and is quieted.
        let mut cpu = Cpu::default();
        mem.0[..4].copy_from_slice(&0x7f80_0001u32.to_le_bytes());
        run_with(&mut cpu, &mut mem, &fld);
        assert_eq!(cpu.x87.status & (IE | DE), IE);
        let v = st(&cpu, 0).unwrap();
        assert_eq!(v.classify(), x87::Class::QNaN);

        // Memory arithmetic with a denormal source sets DE too.
        let mut cpu = Cpu::default();
        mem.0[..4].copy_from_slice(&1u32.to_le_bytes());
        run_with(&mut cpu, &mut mem, &[0xd9, 0xe8, 0xd8, 0x05, 0x00, 0x20, 0x00, 0x00]); // fld1; fadd dword [0x2000]
        assert_ne!(cpu.x87.status & DE, 0);
    }

    #[test]
    fn fxch_st0_keeps_the_stack() {
        // fldz; fld1; fxch st(0)
        let mut cpu = Cpu::default();
        run(&mut cpu, &[0xd9, 0xee, 0xd9, 0xe8, 0xd9, 0xc8]);
        assert_eq!(st(&cpu, 0), Some(F80::ONE));
        assert_eq!(st(&cpu, 1), Some(F80::ZERO));
        // fxch st(1) swaps.
        run(&mut cpu, &[0xd9, 0xc9]);
        assert_eq!(st(&cpu, 0), Some(F80::ZERO));
    }

    #[test]
    fn stores_write_and_pop() {
        // fld1; fstp dword [0x2000]
        let mut cpu = Cpu::default();
        let mut mem = Buf([0; 64]);
        run_with(&mut cpu, &mut mem, &[0xd9, 0xe8, 0xd9, 0x1d, 0x00, 0x20, 0x00, 0x00]);
        assert_eq!(&mem.0[..4], &1.0f32.to_bits().to_le_bytes());
        assert_eq!(st(&cpu, 0), None);
    }
}
