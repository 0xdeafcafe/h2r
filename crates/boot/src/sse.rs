//! A stand-in for the SSE floating-point instructions crates/lift does not
//! lower yet (it lowers SSE moves, shuffles and logic only), for the boot
//! fallback. Host `f32` arithmetic is IEEE round-to-nearest like the guest
//! at its default MXCSR; FTZ is honoured on results. `rcp`/`rsqrt` return
//! the exact quotient rather than the hardware's 12-bit estimate, and NaN
//! propagation follows the host. See docs/FLOAT.md for the real design.

use iced_x86::{Instruction, MemorySize, Mnemonic, OpKind, Register};
use ir::interp::Cpu;

use crate::x87::{Error, Mem};

fn xmm(r: Register) -> Option<usize> {
    let n = r as u32;
    let b = Register::XMM0 as u32;
    (b..b + 8).contains(&n).then(|| (n - b) as usize)
}

fn gpr(r: Register) -> Option<usize> {
    use Register::*;
    Some(match r {
        EAX => 0,
        ECX => 1,
        EDX => 2,
        EBX => 3,
        ESP => 4,
        EBP => 5,
        ESI => 6,
        EDI => 7,
        _ => return Option::None,
    })
}

fn ea(cpu: &Cpu, ins: &Instruction) -> u32 {
    let r = |r: Register| gpr(r).map_or(0, |i| cpu.gpr[i]);
    let mut a = r(ins.memory_base())
        .wrapping_add(r(ins.memory_index()).wrapping_mul(ins.memory_index_scale()))
        .wrapping_add(ins.memory_displacement32());
    if ins.memory_segment() == Register::FS {
        a = a.wrapping_add(cpu.fs_base);
    }
    a
}

fn lanes(v: u128) -> [f32; 4] {
    std::array::from_fn(|i| f32::from_bits((v >> (32 * i)) as u32))
}

fn pack(l: [f32; 4]) -> u128 {
    l.iter().enumerate().fold(0u128, |a, (i, f)| a | ((f.to_bits() as u128) << (32 * i)))
}

fn ftz(cpu: &Cpu, v: f32) -> f32 {
    if cpu.mxcsr & (1 << 15) != 0 && v.is_subnormal() { if v.is_sign_negative() { -0.0 } else { 0.0 } } else { v }
}

fn round(cpu: &Cpu, v: f32, truncate: bool) -> i32 {
    let r = if truncate {
        v.trunc()
    } else {
        match (cpu.mxcsr >> 13) & 3 {
            0 => v.round_ties_even(),
            1 => v.floor(),
            2 => v.ceil(),
            _ => v.trunc(),
        }
    };
    if r.is_nan() || r >= 2147483648.0 || r < -2147483648.0 { i32::MIN } else { r as i32 }
}

fn cmp(pred: u8, a: f32, b: f32) -> bool {
    match pred & 7 {
        0 => a == b,
        1 => a < b,
        2 => a <= b,
        3 => a.is_nan() || b.is_nan(),
        4 => a != b,
        5 => !(a < b),
        6 => !(a <= b),
        _ => !(a.is_nan() || b.is_nan()),
    }
}

/// Execute one SSE floating-point (or SSE-MMX conversion) instruction.
pub fn exec(cpu: &mut Cpu, mem: &mut dyn Mem, ins: &Instruction) -> Result<u32, Error> {
    use Mnemonic as M;
    let m = ins.mnemonic();
    let next = ins.next_ip32();
    let src_kind = if ins.op_count() > 1 { ins.op_kind(1) } else { OpKind::Register };
    // Source operand as 128 bits (memory reads take the operand's size).
    let read_src = |cpu: &Cpu, mem: &mut dyn Mem| -> Result<u128, Error> {
        match src_kind {
            OpKind::Memory => {
                let n = match ins.memory_size() {
                    MemorySize::Float32 | MemorySize::Int32 | MemorySize::UInt32 => 4,
                    MemorySize::Float64 | MemorySize::Int64 | MemorySize::UInt64 | MemorySize::Packed64_Float32 => 8,
                    _ => 16,
                };
                let mut b = [0u8; 16];
                mem.read(ea(cpu, ins), &mut b[..n]).map_err(Error::Memory)?;
                Ok(u128::from_le_bytes(b))
            }
            _ => {
                let r = ins.op_register(1);
                if let Some(x) = xmm(r) {
                    Ok(cpu.xmm[x])
                } else if let Some(g) = gpr(r) {
                    Ok(cpu.gpr[g] as u128)
                } else if (Register::MM0..=Register::MM7).contains(&r) {
                    let i = r as usize - Register::MM0 as usize;
                    Ok(u64::from_le_bytes(cpu.x87.st[i][..8].try_into().unwrap()) as u128)
                } else {
                    Err(Error::Unsupported(format!("{m:?} source {r:?}")))
                }
            }
        }
    };
    let dst_x = xmm(ins.op_register(0));
    let scalar_op = |cpu: &mut Cpu, mem: &mut dyn Mem, f: &dyn Fn(f32, f32) -> f32| -> Result<(), Error> {
        let d = dst_x.ok_or_else(|| Error::Unsupported(format!("{m:?} dst")))?;
        let s = lanes(read_src(cpu, mem)?)[0];
        let mut l = lanes(cpu.xmm[d]);
        l[0] = ftz(cpu, f(l[0], s));
        cpu.xmm[d] = pack(l);
        Ok(())
    };
    let packed_op = |cpu: &mut Cpu, mem: &mut dyn Mem, f: &dyn Fn(f32, f32) -> f32| -> Result<(), Error> {
        let d = dst_x.ok_or_else(|| Error::Unsupported(format!("{m:?} dst")))?;
        let s = lanes(read_src(cpu, mem)?);
        let mut l = lanes(cpu.xmm[d]);
        for i in 0..4 {
            l[i] = ftz(cpu, f(l[i], s[i]));
        }
        cpu.xmm[d] = pack(l);
        Ok(())
    };
    let min = |a: f32, b: f32| if a < b { a } else { b };
    let max = |a: f32, b: f32| if a > b { a } else { b };
    match m {
        M::Addss => scalar_op(cpu, mem, &|a, b| a + b)?,
        M::Subss => scalar_op(cpu, mem, &|a, b| a - b)?,
        M::Mulss => scalar_op(cpu, mem, &|a, b| a * b)?,
        M::Divss => scalar_op(cpu, mem, &|a, b| a / b)?,
        M::Minss => scalar_op(cpu, mem, &min)?,
        M::Maxss => scalar_op(cpu, mem, &max)?,
        M::Sqrtss => scalar_op(cpu, mem, &|_, b| b.sqrt())?,
        M::Rcpss => scalar_op(cpu, mem, &|_, b| 1.0 / b)?,
        M::Rsqrtss => scalar_op(cpu, mem, &|_, b| 1.0 / b.sqrt())?,
        M::Addps => packed_op(cpu, mem, &|a, b| a + b)?,
        M::Subps => packed_op(cpu, mem, &|a, b| a - b)?,
        M::Mulps => packed_op(cpu, mem, &|a, b| a * b)?,
        M::Divps => packed_op(cpu, mem, &|a, b| a / b)?,
        M::Minps => packed_op(cpu, mem, &min)?,
        M::Maxps => packed_op(cpu, mem, &max)?,
        M::Sqrtps => packed_op(cpu, mem, &|_, b| b.sqrt())?,
        M::Rcpps => packed_op(cpu, mem, &|_, b| 1.0 / b)?,
        M::Rsqrtps => packed_op(cpu, mem, &|_, b| 1.0 / b.sqrt())?,
        M::Cmpss | M::Cmpps => {
            let d = dst_x.ok_or_else(|| Error::Unsupported("cmp dst".into()))?;
            let pred = ins.immediate8();
            let s = lanes(read_src(cpu, mem)?);
            let mut v = cpu.xmm[d];
            let l = lanes(v);
            let n = if m == M::Cmpss { 1 } else { 4 };
            for i in 0..n {
                let mask = if cmp(pred, l[i], s[i]) { 0xffff_ffffu128 } else { 0 };
                v = (v & !(0xffff_ffffu128 << (32 * i))) | (mask << (32 * i));
            }
            cpu.xmm[d] = v;
        }
        M::Comiss | M::Ucomiss => {
            let d = dst_x.ok_or_else(|| Error::Unsupported("comiss dst".into()))?;
            let (a, b) = (lanes(cpu.xmm[d])[0], lanes(read_src(cpu, mem)?)[0]);
            let (zf, pf, cf) = if a.is_nan() || b.is_nan() {
                (1, 1, 1)
            } else if a > b {
                (0, 0, 0)
            } else if a < b {
                (0, 0, 1)
            } else {
                (1, 0, 0)
            };
            let mask: u32 = 1 | (1 << 2) | (1 << 4) | (1 << 6) | (1 << 7) | (1 << 11);
            cpu.eflags = (cpu.eflags & !mask) | (zf << 6) | (pf << 2) | cf;
            cpu.flags_valid |= mask;
        }
        M::Cvtsi2ss => {
            let d = dst_x.ok_or_else(|| Error::Unsupported("cvtsi2ss dst".into()))?;
            let v = read_src(cpu, mem)? as u32 as i32;
            let mut l = lanes(cpu.xmm[d]);
            l[0] = v as f32;
            cpu.xmm[d] = pack(l);
        }
        M::Cvtss2si | M::Cvttss2si => {
            let g = gpr(ins.op_register(0)).ok_or_else(|| Error::Unsupported("cvtss2si dst".into()))?;
            let v = lanes(read_src(cpu, mem)?)[0];
            cpu.gpr[g] = round(cpu, v, m == M::Cvttss2si) as u32;
        }
        M::Cvtpi2ps => {
            let d = dst_x.ok_or_else(|| Error::Unsupported("cvtpi2ps dst".into()))?;
            let s = read_src(cpu, mem)?;
            let mut l = lanes(cpu.xmm[d]);
            l[0] = (s as u32 as i32) as f32;
            l[1] = ((s >> 32) as u32 as i32) as f32;
            cpu.xmm[d] = pack(l);
        }
        M::Cvtps2pi | M::Cvttps2pi => {
            let r = ins.op_register(0);
            if !(Register::MM0..=Register::MM7).contains(&r) {
                return Err(Error::Unsupported("cvtps2pi dst".into()));
            }
            let s = lanes(read_src(cpu, mem)?);
            let t = m == M::Cvttps2pi;
            let v = (round(cpu, s[0], t) as u32 as u64) | ((round(cpu, s[1], t) as u32 as u64) << 32);
            let i = r as usize - Register::MM0 as usize;
            cpu.x87.st[i][..8].copy_from_slice(&v.to_le_bytes());
            cpu.x87.st[i][8] = 0xff;
            cpu.x87.st[i][9] = 0xff;
            cpu.x87.tag = 0;
            cpu.x87.status &= !(7 << 11);
        }
        M::Prefetchnta | M::Prefetcht0 | M::Prefetcht1 | M::Prefetcht2 | M::Sfence => {}
        other => return Err(Error::Unsupported(format!("sse {other:?}"))),
    }
    Ok(next)
}
