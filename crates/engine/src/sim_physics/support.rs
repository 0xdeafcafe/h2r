//! Fetch the cached support simplex from both shapes and transform the
//! second shape's packed support vertices into the first shape's space.
use super::*;
use super::havok::{put_vector, simd_fault, splat, vector};

pub(super) const SITES: &[u32] = &[0x3048e0,0x3048f8];
pub(super) const REST: &[u32] = &[0x304970];
const INITIAL_TOLERANCE_OPERAND: u32 = 0x0030_48d9;

pub(super) fn vertices(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
    if pc == points::HAVOK_SUPPORT_VERTICES.base {
        push(c,g,c.ebp)?;
        c.ebp = c.esp;
        let aligned = c.esp & !15;
        c.esp = arithmetic(c,aligned,116,true);
        push(c,g,c.ebx)?;
        c.ebx = g.u32(c.ebp+20)?;
        c.eax = g.u8(c.ebx+8)? as i8 as i32 as u32;
        push(c,g,c.esi)?;
        c.esi = c.ecx;
        g.set_u32(c.esi,c.eax)?;
        c.ecx = g.u8(c.ebx+9)? as i8 as i32 as u32;
        push(c,g,c.edi)?;
        push(c,g,c.esi+32)?;
        c.eax = g.u32(c.esi)?;
        c.edi = c.esi+4;
        g.set_u32(c.edi,c.ecx)?;
        c.ecx = g.u32(c.ebp+8)?;
        c.edx = g.u32(c.ecx)?;
        push(c,g,c.eax)?;
        push(c,g,c.ebx)?;
        g.set_u32(c.esp+40,0)?;
        let tolerance = g.u32(INITIAL_TOLERANCE_OPERAND)?;
        g.set_u32(c.esp+32,tolerance)?;
        let target = g.u32(c.edx+52)?;
        return g.call(c,target,SITES[0]);
    }
    if pc == SITES[0] {
        c.ecx = g.u32(c.ebp+12)?;
        c.edx = g.u32(c.ecx)?;
        push(c,g,c.esi+224)?;
        c.eax = g.u32(c.edi)?;
        push(c,g,c.eax)?;
        c.eax = c.ebx.wrapping_add(2u32.wrapping_mul(g.u32(c.esi)?));
        push(c,g,c.eax)?;
        let target = g.u32(c.edx+52)?;
        return g.call(c,target,SITES[1]);
    }
    let count = g.u32(c.edi)?;
    let carry = c.eflags & 1;
    let carry_defined = c.eflags_defined & 1;
    c.eax = arithmetic(c,count,1,true);
    c.eflags = c.eflags & !1 | carry;
    c.eflags_defined = c.eflags_defined & !1 | carry_defined;
    c.edx = c.esi+160;
    c.ecx = c.esi+224;
    if c.eax as i32 >= 0 {
        c.edx = c.edx.wrapping_sub(c.ecx);
        c.ebx = count;
        c.eax = g.u32(c.ebp+16)?;
        loop {
            let v = vector(g,c.ecx)?;
            let m = &mut c.mxcsr;
            let x = simd::mulps(m,vector(g,c.eax)?,splat(v as u32)).map_err(simd_fault)?;
            let y = simd::mulps(m,vector(g,c.eax+16)?,splat((v>>32) as u32)).map_err(simd_fault)?;
            let z = simd::mulps(m,vector(g,c.eax+32)?,splat((v>>64) as u32)).map_err(simd_fault)?;
            let t = vector(g,c.eax+48)?;
            let zt = simd::addps(m,z,t).map_err(simd_fault)?;
            let xy = simd::addps(m,x,y).map_err(simd_fault)?;
            let result = simd::addps(m,xy,zt).map_err(simd_fault)?;
            put_vector(g,c.edx.wrapping_add(c.ecx),result)?;
            for (dst,val) in c.xmm.iter_mut().zip([zt,result,t,y]) { *dst = val.to_le_bytes(); }
            c.ecx = arithmetic(c,c.ecx,16,false);
            let carry = c.eflags & 1;
            c.ebx = arithmetic(c,c.ebx,1,true);
            c.eflags = c.eflags & !1 | carry;
            if c.ebx == 0 { break; }
        }
    }
    g.set_u32(c.esp+32,0)?;
    Ok(TransferAbi::resume(REST[0]))
}
