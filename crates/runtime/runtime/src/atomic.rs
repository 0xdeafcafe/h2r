//! Integer atomic semantics. Call these methods through one runtime memory
//! guard; that guard covers validation, memory, CPU flags and results.
use crate::{
    CpuAbi, Guest, GuestFault,
    guest::{Access, FaultReason},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum AtomicOp {
    Add = 1,
    Adc,
    Sub,
    Sbb,
    And,
    Or,
    Xor,
    Inc,
    Dec,
    Neg,
    Not,
    Bts,
    Btr,
    Btc,
}
impl AtomicOp {
    pub(crate) fn from_tag(tag: u32) -> Option<Self> {
        Some(match tag {
            1 => Self::Add,
            2 => Self::Adc,
            3 => Self::Sub,
            4 => Self::Sbb,
            5 => Self::And,
            6 => Self::Or,
            7 => Self::Xor,
            8 => Self::Inc,
            9 => Self::Dec,
            10 => Self::Neg,
            11 => Self::Not,
            12 => Self::Bts,
            13 => Self::Btr,
            14 => Self::Btc,
            _ => return None,
        })
    }
}
const CF: u32 = 1;
const PF: u32 = 4;
const AF: u32 = 0x10;
const ZF: u32 = 0x40;
const SF: u32 = 0x80;
const OF: u32 = 0x800;
const FLAGS: u32 = CF | PF | AF | ZF | SF | OF;

fn arithmetic(cpu: &mut CpuAbi, a: u32, b: u32, carry: u32, bits: u32, subtract: bool) -> u32 {
    let mask = u32::MAX >> (32 - bits);
    let a = a & mask;
    let b = b & mask;
    let result = if subtract {
        a.wrapping_sub(b).wrapping_sub(carry)
    } else {
        a.wrapping_add(b).wrapping_add(carry)
    } & mask;
    let cf = if subtract {
        (a as u64) < b as u64 + carry as u64
    } else {
        a as u64 + b as u64 + carry as u64 > mask as u64
    };
    let sign = 1u32 << (bits - 1);
    let of = (if subtract {
        (a ^ b) & (a ^ result)
    } else {
        !(a ^ b) & (a ^ result)
    }) & sign
        != 0;
    let flags = u32::from(cf)
        | (u32::from(of) * OF)
        | (u32::from((a ^ b ^ result) & 0x10 != 0) * AF)
        | status_flags(result, sign);
    cpu.eflags = (cpu.eflags & !FLAGS) | flags;
    cpu.eflags_defined |= FLAGS;
    result
}
fn status_flags(value: u32, sign: u32) -> u32 {
    (u32::from(value == 0) * ZF)
        | (u32::from(value & sign != 0) * SF)
        | (u32::from((value as u8).count_ones().is_multiple_of(2)) * PF)
}
fn width_mask(addr: u32, width: u32) -> Result<u32, GuestFault> {
    if !matches!(width, 1 | 2 | 4) {
        return Err(GuestFault {
            addr,
            len: width as usize,
            access: Access::Write,
            reason: FaultReason::InvalidWidth,
        });
    }
    Ok(u32::MAX >> (32 - width * 8))
}
impl Guest {
    fn atomic_read(&self, addr: u32, width: u32) -> Result<u32, GuestFault> {
        self.check(addr, width as usize, Access::Write, 1)?;
        let mut bytes = [0; 4];
        self.read_bytes(addr, &mut bytes[..width as usize], 1)?;
        Ok(u32::from_le_bytes(bytes))
    }
    fn atomic_write(&mut self, addr: u32, width: u32, value: u32) -> Result<(), GuestFault> {
        self.write_bytes(addr, &value.to_le_bytes()[..width as usize], 1)
    }
    /// LOCK arithmetic/logic returns the old memory operand. INC/DEC preserve
    /// CF, NOT preserves all flags, and logical operations invalidate AF.
    pub fn atomic_rmw(
        &mut self,
        cpu: &mut CpuAbi,
        addr: u32,
        width: u32,
        op: AtomicOp,
        operand: u32,
    ) -> Result<u32, GuestFault> {
        let mask = width_mask(addr, width)?;
        if matches!(op, AtomicOp::Bts | AtomicOp::Btr | AtomicOp::Btc) && width == 1 {
            return Err(GuestFault {
                addr,
                len: width as usize,
                access: Access::Write,
                reason: FaultReason::InvalidWidth,
            });
        }
        let old = self.atomic_read(addr, width)?;
        let carry = cpu.eflags & CF;
        let cf_valid = cpu.eflags_defined & CF;
        let bits = width * 8;
        let value = match op {
            AtomicOp::Add => arithmetic(cpu, old, operand, 0, bits, false),
            AtomicOp::Adc => arithmetic(cpu, old, operand, carry, bits, false),
            AtomicOp::Sub => arithmetic(cpu, old, operand, 0, bits, true),
            AtomicOp::Sbb => arithmetic(cpu, old, operand, carry, bits, true),
            AtomicOp::Inc | AtomicOp::Dec => {
                let result = arithmetic(cpu, old, 1, 0, bits, op == AtomicOp::Dec);
                cpu.eflags = (cpu.eflags & !CF) | carry;
                cpu.eflags_defined = (cpu.eflags_defined & !CF) | cf_valid;
                result
            }
            AtomicOp::Neg => arithmetic(cpu, 0, old, 0, bits, true),
            AtomicOp::Not => !old & mask,
            AtomicOp::Bts | AtomicOp::Btr | AtomicOp::Btc => {
                let bit = 1 << (operand % bits);
                cpu.eflags = (cpu.eflags & !CF) | u32::from(old & bit != 0);
                cpu.eflags &= !(OF | SF | AF | PF);
                cpu.eflags_defined = (cpu.eflags_defined | CF) & !(OF | SF | AF | PF);
                match op {
                    AtomicOp::Bts => old | bit,
                    AtomicOp::Btr => old & !bit,
                    _ => old ^ bit,
                }
            }
            AtomicOp::And | AtomicOp::Or | AtomicOp::Xor => {
                let result = match op {
                    AtomicOp::And => old & operand,
                    AtomicOp::Or => old | operand,
                    _ => old ^ operand,
                } & mask;
                cpu.eflags = (cpu.eflags & !FLAGS) | status_flags(result, 1 << (bits - 1));
                cpu.eflags_defined = (cpu.eflags_defined | FLAGS) & !AF;
                result
            }
        };
        if matches!(op, AtomicOp::Adc | AtomicOp::Sbb) && cf_valid == 0 {
            cpu.eflags_defined &= !FLAGS;
        }
        // Validation above covered both permissions and every byte. The guard
        // prevents a mapping change between that validation and this write.
        self.atomic_write(addr, width, value)?;
        Ok(old)
    }
    pub fn atomic_xchg(&mut self, addr: u32, width: u32, value: u32) -> Result<u32, GuestFault> {
        width_mask(addr, width)?;
        let old = self.atomic_read(addr, width)?;
        self.atomic_write(addr, width, value)?;
        Ok(old)
    }
    pub fn atomic_cmpxchg(
        &mut self,
        cpu: &mut CpuAbi,
        addr: u32,
        width: u32,
        replacement: u32,
    ) -> Result<u32, GuestFault> {
        let mask = width_mask(addr, width)?;
        let old = self.atomic_read(addr, width)?;
        let expected = cpu.eax & mask;
        arithmetic(cpu, expected, old, 0, width * 8, true);
        if expected == old {
            self.atomic_write(addr, width, replacement)?;
        } else {
            cpu.eax = (cpu.eax & !mask) | old;
        }
        Ok(old)
    }
    pub fn atomic_cmpxchg8b(&mut self, cpu: &mut CpuAbi, addr: u32) -> Result<(), GuestFault> {
        self.check(addr, 8, Access::Write, 1)?;
        let old = self.u64(addr)?;
        let expected = ((cpu.edx as u64) << 32) | cpu.eax as u64;
        if old == expected {
            self.set_u64(addr, ((cpu.ecx as u64) << 32) | cpu.ebx as u64)?;
            cpu.eflags |= ZF;
        } else {
            cpu.eax = old as u32;
            cpu.edx = (old >> 32) as u32;
            cpu.eflags &= !ZF;
        }
        cpu.eflags_defined |= ZF;
        Ok(())
    }
}
