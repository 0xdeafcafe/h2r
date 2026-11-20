//! Aligned Havok vectors and schema streams. All dispatch tables, scaling
//! factors and tolerances belong to the mapped image or the caller.
use super::*;
use super::spatial::{finish_fpu, fp_error, load_fpu};
use crate::light_constants::Error;

pub(super) fn vector(g: &Guest, p: u32) -> Result<u128> {
    let mut v = [0; 16];
    g.read_bytes(p, &mut v, 16)?;
    Ok(u128::from_le_bytes(v))
}
pub(super) fn put_vector(g: &mut Guest, p: u32, v: u128) -> Result<()> {
    g.write_bytes(p, &v.to_le_bytes(), 16)
}
pub(super) fn splat(v: u32) -> u128 {
    u128::from(v) * (u128::MAX / u128::from(u32::MAX))
}
pub(super) fn simd_fault(_: simd::Xm) -> GuestFault { fp_error(Error::Simd) }

const READY: u32 = 0x0048_0c60;
const SOLVE_DISPATCH: u32 = 0x0031_e6a4;
const WRITE_DISPATCH: u32 = 0x0031_e704;
const WRITE_CLASS: u32 = 0x0031_e724;
const SCHEMA_LOOP: u32 = 0x31c9b4;
const WRITE_LOOP: u32 = 0x31e510;
pub(super) const SOLVER_REST: &[u32] = &[SCHEMA_LOOP, WRITE_LOOP, 0x31cfa5];
pub(super) const SOLVER_SITES: &[u32] = &[0x31c95d, 0x31e55f, 0x31e590, 0x31e5d3, 0x31e626, 0x31e683];

fn leave(c: &mut CpuAbi, g: &Guest, value: u32) -> Result<TransferAbi> {
    c.edi = pop(c,g)?;
    c.esi = pop(c,g)?;
    c.ebx = pop(c,g)?;
    c.esp = c.ebp;
    c.ebp = pop(c,g)?;
    c.eax = value;
    g.ret(c,0)
}

pub(super) fn solve(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
    let entry = pc == points::HAVOK_SOLVER.base;
    if entry {
        push(c,g,c.ebp)?;
        c.ebp = c.esp;
        c.esp = (c.esp & !15).wrapping_sub(180);
        c.eax = byte(c.eax, g.u8(READY)?);
        logic(c,c.eax & 255,128);
        push(c,g,c.ebx)?;
        push(c,g,c.esi)?;
        push(c,g,c.edi)?;
        if c.eax as u8 == 0 { return g.call(c,0x31c7d0,0x31c95d); }
    }
    if pc == 0x31c95d {
        c.eax = byte(c.eax,g.u8(READY)?);
        logic(c,c.eax & 255,128);
        if c.eax as u8 == 0 { logic(c,0,1<<31); return leave(c,g,0); }
    }
    let sp = c.esp;
    let mut repeated = None;
    if entry || pc == 0x31c95d {
        c.eax = g.u32(c.ebp+8)?;
        c.ecx = g.u32(c.eax+28)?;
        logic(c,c.ecx,1<<31);
        g.set_u32(sp+44,0)?;
        if c.ecx as i32 > 0 {
            c.ecx = g.u32(c.eax+4)?;
            c.edx = g.u32(c.eax+8)?;
            c.ebx = g.u32(c.ebp+12)?;
            c.edi = g.u32(c.ebp+20)?;
            g.set_u32(sp+48,c.ecx)?;
            g.set_u32(sp+40,c.edx)?;
            c.ecx = g.u32(c.ebx+12)?;
            c.eax = g.u32(c.ebx+16)?;
            c.esi = g.u32(c.ebx+4)?;
            g.set_u32(sp+36,c.ecx)?;
            g.set_u32(sp+12,c.eax)?;
            c.ebx = arithmetic(c,c.ebx,24,false);
            return linear_rows(c,g);
        }
        c.edi = g.u32(c.ebp+20)?;
        c.esi = g.u32(c.ebp+12)?;
        output_header(c,g)?;
    } else {
        let (impulses, schema) = match pc {
            0x31e55f => (8,8),
            0x31e626 => (8,20),
            0x31e683 => (12,24),
            _ => {
                let size = u32::from(g.u8(c.esi+1)?);
                let next = g.u8(c.esi.wrapping_add(size))? as i8 as i32 as u32;
                if next == g.u32(sp+40)? {
                    repeated = Some(if pc == 0x31e590 { 0x31e567 } else { 0x31e5aa });
                }
                (4,size)
            }
        };
        c.edi = c.edi.wrapping_add(impulses);
        c.esi = c.esi.wrapping_add(schema);
    }
    // All output-producing commands call the row advance helper. Bound
    // runs of pure skip/header commands at the image's polling selector.
    for _ in 0..64 {
        let repeat = repeated.take();
        let target = if let Some(target) = repeat { target } else {
            c.ecx = g.u8(c.esi)? as i8 as i32 as u32;
            arithmetic(c,c.ecx,16,true);
            g.set_u32(sp+40,c.ecx)?;
            if c.ecx > 16 { return Ok(TransferAbi::resume(WRITE_LOOP)); }
            c.edx = u32::from(g.u8(WRITE_CLASS+c.ecx)?);
            g.u32(WRITE_DISPATCH+4*c.edx)?
        };
        match target {
            0x31e696 => return leave(c,g,1),
            0x31e4f7 => output_header(c,g)?,
            0x31e68b => {
                c.ecx = u32::from(g.u8(c.esi+1)?);
                c.esi = arithmetic(c,c.esi,c.ecx,false);
            }
            _ => return write_impulses(c,g,target,repeat.is_some()),
        }
    }
    Ok(TransferAbi::resume(WRITE_LOOP))
}

fn output_header(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    c.ecx = g.u32(c.esi+20)?;
    c.eax = g.u32(c.esi+4)?;
    c.ebx = g.u32(c.esi+8)?;
    g.set_u32(c.esp+36,c.ecx)?;
    c.esi = arithmetic(c,c.esi,24,false);
    Ok(())
}

fn write_impulses(c: &mut CpuAbi, g: &mut Guest, form: u32, repeat: bool) -> Result<TransferAbi> {
    let sp = c.esp;
    let explicit = matches!(form, 0x31e5ed | 0x31e631);
    let (count, rows, angular, store_pc, site) = match form {
        0x31e567 => (1,1,false,0x31e586,0x31e590),
        0x31e5aa => (1,1,true,0x31e5c7,0x31e5d3),
        0x31e52a => (2,2,false,0x31e555,0x31e55f),
        0x31e5ed => (2,2,false,0x31e61c,0x31e626),
        0x31e631 => (3,1,true,0x31e678,0x31e683),
        _ => return Ok(TransferAbi::resume(WRITE_LOOP)),
    };
    let mut dest = if explicit { g.u32(c.esi+4)? } else { c.ebx };
    let mut x = load_fpu(c);
    let mut scale = if repeat { g.u32(sp+16)? } else { g.u32(c.ebp+8)?.wrapping_add(16) };
    if count == 1 && !repeat { g.set_u32(sp+16,scale)?; }
    let mut last = dest;
    for i in 0..count {
        let impulse = g.u32(c.edi+4*i)?;
        g.set_u32(dest,impulse)?;
        let rhs = match i { 2 => 124, 1 => 60, _ if count == 1 && angular => 28, _ => 12 };
        x.fld(g.u32(c.eax+rhs)?).map_err(fp_error)?;
        if explicit && i != 0 { scale = g.u32(sp+16)?; }
        x.fmul_m(g.u32(scale)?).map_err(fp_error)?;
        if i == 0 {
            if explicit { g.set_u32(sp+16,scale)?; }
            push(c,g,rows)?;
        }
        last = dest+4;
        g.set_u32(last,x.fstp_m32().map_err(fp_error)?)?;
        if explicit {
            c.edx = scale;
            if i + 1 < count {
                let stride = g.u32(c.esi+16)?;
                dest = arithmetic(c,dest,stride,false);
            }
            if i == 1 && count == 3 { c.ebx = 0; logic(c,0,1<<31); }
        } else {
            let stride = g.u32(sp+36)?;
            dest = arithmetic(c,dest,stride,false);
            c.ebx = dest;
            c.edx = if angular { scale } else { stride };
        }
    }
    c.ecx = if count == 3 { c.eax.wrapping_add(96) } else { c.eax };
    finish_fpu(c,g,x,store_pc,last)?;
    g.call(c,if angular { 0x31c520 } else { 0x31c530 },site)
}

// Consecutive unilateral linear constraints share the two accumulators.
// Other schema types enter the original command interpreter at its poll.
fn linear_rows(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    let mut continuing = false;
    for _ in 0..64 {
        if !continuing {
            let command = g.u8(c.ebx)? as i8 as i32 as u32;
            if command > 16 || g.u32(SOLVE_DISPATCH+4*command)? != 0x31cf97 {
                return Ok(TransferAbi::resume(SCHEMA_LOOP));
            }
            c.eax = g.u32(c.esp+12)?;
            c.edx = c.eax.wrapping_add(16);
            c.eax = c.eax.wrapping_add(32);
            g.set_u32(c.esp+20,c.eax)?;
        }
        apply_linear(c,g)?;
        c.eax = byte(c.eax,g.u8(c.ebx+4)?);
        c.ebx = c.ebx.wrapping_add(4);
        c.esi = c.esi.wrapping_add(48);
        c.edi = c.edi.wrapping_add(4);
        let a = c.eax as u8;
        let result = a.wrapping_sub(2);
        c.eflags = (c.eflags & !ARITH) | status(u32::from(result),128)
            | u32::from(a < 2) | u32::from((a ^ 2 ^ result) & 16)
            | (u32::from((a ^ 2) & (a ^ result) & 128 != 0) << 11);
        c.eflags_defined |= ARITH;
        continuing = a == 2;
        if !continuing { return Ok(TransferAbi::resume(SCHEMA_LOOP)); }
    }
    Ok(TransferAbi::resume(0x31cfa5))
}

fn apply_linear(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    let (a, b_linear, row, sp) = (c.ecx, c.edx, c.esi, c.esp);
    let b_angular = g.u32(sp+20)?;
    let mut x = load_fpu(c);
    let mut m = c.mxcsr;
    let mut regs = [0u128; 5];
    x.fld(g.u32(row+12)?).map_err(fp_error)?;
    let rel = simd::subps(&mut m,vector(g,a+16)?,vector(g,b_linear)?).map_err(simd_fault)?;
    let ang_a = simd::mulps(&mut m,vector(g,a+32)?,vector(g,row+16)?).map_err(simd_fault)?;
    regs[3] = simd::mulps(&mut m,vector(g,b_angular)?,vector(g,row+32)?).map_err(simd_fault)?;
    regs[4] = vector(g,row)?;
    let lin = simd::mulps(&mut m,rel,regs[4]).map_err(simd_fault)?;
    let ang = simd::addps(&mut m,ang_a,regs[3]).map_err(simd_fault)?;
    regs[0] = simd::addps(&mut m,ang,lin).map_err(simd_fault)?;
    regs[1] = simd::addss(&mut m,splat((regs[0]>>32) as u32),regs[0] as u32).map_err(simd_fault)?;
    regs[2] = simd::addss(&mut m,splat((regs[0]>>64) as u32),regs[1] as u32).map_err(simd_fault)?;
    g.set_u32(sp+80,regs[2] as u32)?;
    c.eax = g.u32(c.ebp+8)?;
    x.fmul_m(g.u32(c.eax+4)?).map_err(fp_error)?;
    x.fld(g.u32(sp+80)?).map_err(fp_error)?;
    x.fmul_m(g.u32(c.eax+8)?).map_err(fp_error)?;
    x.arith_reg(x87::abi::arith_op::SUB,1,0,true).map_err(fp_error)?;
    let floor = 0x0045_dbd8;
    let cmp = x87::abi::cmp(&mut x.0,0,x87::abi::src_kind::F32,u64::from(g.u32(floor)?),0,false);
    crate::light_constants::X87::ok(cmp).map_err(fp_error)?;
    c.eax = (c.eax & !0xffff) | u32::from(x.0.status);
    let (last, operand) = if x.0.status & 0x4100 == 0 {
        x.fmul_m(g.u32(row+28)?).map_err(fp_error)?;
        let mass_a = vector(g,a+48)?;
        let body_b = g.u32(sp+12)?;
        g.set_u32(sp+24,x.fst_m32().map_err(fp_error)?)?;
        let gain = splat(g.u32(sp+24)?);
        regs[0] = simd::mulps(&mut m,gain,mass_a).map_err(simd_fault)?;
        regs[1] = simd::mulps(&mut m,gain,vector(g,body_b+48)?).map_err(simd_fault)?;
        c.eax = g.u32(sp+20)?;
        let delta_a = simd::mulps(&mut m,splat((regs[0]>>96) as u32),regs[4]).map_err(simd_fault)?;
        regs[3] = simd::mulps(&mut m,splat((regs[1]>>96) as u32),regs[4]).map_err(simd_fault)?;
        regs[4] = simd::addps(&mut m,vector(g,a+16)?,delta_a).map_err(simd_fault)?;
        put_vector(g,a+16,regs[4])?;
        let vb = simd::subps(&mut m,vector(g,b_linear)?,regs[3]).map_err(simd_fault)?;
        put_vector(g,b_linear,vb)?;
        regs[0] = simd::mulps(&mut m,regs[0],vector(g,row+16)?).map_err(simd_fault)?;
        regs[1] = simd::mulps(&mut m,regs[1],vector(g,row+32)?).map_err(simd_fault)?;
        regs[2] = simd::addps(&mut m,vector(g,a+32)?,regs[0]).map_err(simd_fault)?;
        put_vector(g,a+32,regs[2])?;
        regs[0] = simd::addps(&mut m,vector(g,c.eax)?,regs[1]).map_err(simd_fault)?;
        put_vector(g,c.eax,regs[0])?;
        x.fadd_m(g.u32(c.edi)?).map_err(fp_error)?;
        g.set_u32(c.edi,x.fstp_m32().map_err(fp_error)?)?;
        (0x31d08f,c.edi)
    } else {
        x.fstp_st(0).map_err(fp_error)?;
        (0x31d093,floor)
    };
    for (dest,src) in c.xmm.iter_mut().zip(regs) { *dest = src.to_le_bytes(); }
    c.mxcsr = m;
    finish_fpu(c,g,x,last,operand)
}
