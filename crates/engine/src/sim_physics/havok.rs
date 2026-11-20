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
pub(super) const SOLVER_REST: &[u32] = &[SCHEMA_LOOP, WRITE_LOOP, 0x31e0d0, 0x31e124, 0x31e0f0];
pub(super) const SOLVER_SITES: &[u32] = &[0x31c95d, 0x31e04d, 0x31e0ae, 0x31e55f, 0x31e590, 0x31e5d3, 0x31e626, 0x31e683];

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
    if entry || pc == 0x31c95d {
        let params = g.u32(c.ebp+8)?;
        c.ecx = g.u32(params+28)?;
        logic(c,c.ecx,1<<31);
        g.set_u32(sp+44,0)?;
        if c.ecx as i32 > 0 {
            start_iteration(c,g)?;
            return schemas(c,g);
        }
        return begin_output(c,g);
    }
    if pc == 0x31e04d {
        c.ecx = g.u32(sp+36)?;
        c.ebx = c.ebx.wrapping_add(16);
        c.esi = c.eax;
        c.eax = byte(c.eax,g.u8(c.ebx)?);
        c.edi = c.edi.wrapping_add(4);
        return schemas(c,g);
    }
    if pc == 0x31e0ae {
        let size = u32::from(g.u8(c.ebx+1)?);
        c.esp = c.esp.wrapping_add(8);
        c.ebx = arithmetic(c,c.ebx,size,false);
        c.ecx = g.u32(c.esp+36)?;
        return schemas(c,g);
    }
    writeback(c,g,Some(pc))
}

fn begin_output(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    c.edi = g.u32(c.ebp+20)?;
    c.esi = g.u32(c.ebp+12)?;
    output_header(c,g)?;
    writeback(c,g,None)
}

fn writeback(c: &mut CpuAbi, g: &mut Guest, resume: Option<u32>) -> Result<TransferAbi> {
    let sp = c.esp;
    let mut repeated = None;
    if let Some(pc) = resume {
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
    loop {
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
            if i == 1 && count == 3 { c.ebx = 0; logic(c,0,1<<31); }
            if i + 1 < count {
                let stride = g.u32(c.esi+16)?;
                dest = arithmetic(c,dest,stride,false);
            }
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

fn primary_header(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    c.ecx = g.u32(c.ebx+12)?;
    c.eax = g.u32(c.ebx+16)?;
    c.esi = g.u32(c.ebx+4)?;
    g.set_u32(c.esp+36,c.ecx)?;
    g.set_u32(c.esp+12,c.eax)?;
    c.ebx = arithmetic(c,c.ebx,24,false);
    Ok(())
}

fn start_iteration(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    let params = g.u32(c.ebp+8)?;
    c.edx = g.u32(params+8)?;
    g.set_u32(c.esp+48,g.u32(params+4)?)?;
    g.set_u32(c.esp+40,c.edx)?;
    c.ebx = g.u32(c.ebp+12)?;
    c.edi = g.u32(c.ebp+20)?;
    primary_header(c,g)
}

fn schemas(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    let sp = c.esp;
    let mut second_pair = false;
    loop {
        c.edx = c.esi.wrapping_add(80);
        c.eax = g.u8(c.ebx)? as i8 as i32 as u32;
        arithmetic(c,c.eax,16,true);
        if c.eax > 16 { return Ok(TransferAbi::resume(SCHEMA_LOOP)); }
        let form = g.u32(SOLVE_DISPATCH+4*c.eax)?;
        let alternate = second_pair;
        second_pair = false;
        match form {
            0x31c9a0 => primary_header(c,g)?,
            0x31cf97 => {
                let b = g.u32(sp+12)?;
                c.edx = b+16;
                g.set_u32(sp+20,b+32)?;
                apply_linear(c,g)?;
                c.ebx += 4;
                c.esi += 48;
                c.edi += 4;
            }
            0x31e068 | 0x31e08d => {
                let params = g.u32(c.ebp+8)?;
                if form == 0x31e068 {
                    g.set_u32(sp+40,g.u32(params+8)?)?;
                    g.set_u32(sp+48,g.u32(params+4)?)?;
                    g.set_u32(params+8,g.u32(c.ebx+8)?)?;
                    g.set_u32(params+4,g.u32(c.ebx+4)?)?;
                    c.ebx += 12;
                } else {
                    g.set_u32(params+4,g.u32(sp+48)?)?;
                    g.set_u32(params+8,g.u32(sp+40)?)?;
                    c.ebx += 4;
                }
            }
            0x31e0a6 => {
                c.eax = c.ebx+8;
                push(c,g,c.eax)?;
                push(c,g,c.esi)?;
                return g.call(c,g.u32(c.ebx+4)?,0x31e0ae);
            }
            0x31e0c0 => {
                if let Some(site) = super::solver_rows::correct_velocities(c,g)? {
                    return Ok(TransferAbi::resume(site));
                }
                let iteration = g.u32(sp+44)?.wrapping_add(1);
                g.set_u32(sp+44,iteration)?;
                let params = g.u32(c.ebp+8)?;
                if (iteration as i32) >= g.u32(params+28)? as i32 {
                    return begin_output(c,g);
                }
                start_iteration(c,g)?;
            }
            0x31c9c6 | 0x31d532 | 0x31d1fb => {
                super::solver_pairs::coupled(c,g,form,alternate)?;
                second_pair = form == 0x31c9c6 && !alternate;
            }
            _ => if let Some(t) = super::solver_rows::single(c,g,form)? {return Ok(t)},
        }
    }
}

fn apply_linear(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    use super::solver_rows::{Axis, ZERO, apply, compare, project, residual, set};
    let (a,b,row,sp) = (c.ecx,g.u32(c.esp+12)?,c.esi,c.esp);
    let v = project(c,g,a,b,row,false)?;
    let dot = v.xyz as u32;
    set(c,0,v.lanes);
    set(c,1,v.xy);
    set(c,2,v.xyz);
    set(c,3,v.angular_b);
    set(c,4,vector(g,row)?);
    g.set_u32(sp+80,dot)?;
    let params = g.u32(c.ebp+8)?;
    let mut x = load_fpu(c);
    residual(g,&mut x,row+12,dot,params+4,params+8)?;
    if compare(c,&mut x,g.u32(ZERO)?,false,65)?==0 {
        x.fmul_m(g.u32(row+28)?).map_err(fp_error)?;
        let gain = x.fst_m32().map_err(fp_error)?;
        g.set_u32(sp+24,gain)?;
        apply(c,g,&mut x,a,b,row,c.edi,gain,Axis::Linear)?;
        finish_fpu(c,g,x,0x31d08f,c.edi)
    } else {
        x.fstp_st(0).map_err(fp_error)?;
        finish_fpu(c,g,x,0x31d093,ZERO)
    }
}
