//! Jacobian projection, individual constraint impulses and the per-iteration
//! velocity correction pass. A projected velocity rounds to f32 at the SSE
//! spill. The following constraint calculation keeps its x87 precision until
//! the guest explicitly stores an impulse or a Jacobian component.
use super::*;
use super::havok::{put_vector, simd_fault, splat, vector};
use super::spatial::{finish_fpu, fp_error, load_fpu};
use crate::light_constants::X87;
use x87::abi::{arith_op as op, unary_op};

pub(super) const ZERO: u32 = 0x0045_dbd8;
const CORRECTION_DISPATCH: u32 = 0x0031_e6e8;

pub(super) fn set(c: &mut CpuAbi, reg: usize, v: u128) {
    c.xmm[reg] = v.to_le_bytes();
}
fn mul(c: &mut CpuAbi, a: u128, b: u128) -> Result<u128> {
    simd::mulps(&mut c.mxcsr,a,b).map_err(simd_fault)
}
fn add(c: &mut CpuAbi, a: u128, b: u128) -> Result<u128> {
    simd::addps(&mut c.mxcsr,a,b).map_err(simd_fault)
}
fn sub(c: &mut CpuAbi, a: u128, b: u128) -> Result<u128> {
    simd::subps(&mut c.mxcsr,a,b).map_err(simd_fault)
}

pub(super) struct Projection {
    pub lanes: u128,
    pub xy: u128,
    pub xyz: u128,
    pub linear: u128,
    pub angular_b: u128,
}

pub(super) fn project(c: &mut CpuAbi, g: &Guest, a: u32, b: u32, row: u32, angular: bool) -> Result<Projection> {
    let axis = if angular {row} else {row+16};
    let va = mul(c,vector(g,a+32)?,vector(g,axis)?)?;
    let vb = mul(c,vector(g,b+32)?,vector(g,axis+16)?)?;
    let mut lanes = add(c,va,vb)?;
    let mut linear = 0;
    if !angular {
        let velocity = sub(c,vector(g,a+16)?,vector(g,b+16)?)?;
        linear = mul(c,velocity,vector(g,row)?)?;
        lanes = add(c,lanes,linear)?;
    }
    let xy = simd::addss(&mut c.mxcsr,splat((lanes>>32) as u32),lanes as u32).map_err(simd_fault)?;
    let xyz = simd::addss(&mut c.mxcsr,splat((lanes>>64) as u32),xy as u32).map_err(simd_fault)?;
    Ok(Projection {lanes,xy,xyz,linear,angular_b:vb})
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Axis {Linear, LinearUnbounded, Angular, AngularCompact}

/// Apply the rounded factor to the two bodies, then add the still-extended
/// impulse to the previous scalar impulse. B is read after A is written.
pub(super) fn apply(c: &mut CpuAbi, g: &mut Guest, x: &mut X87, a: u32, b: u32,
                    row: u32, impulse: u32, gain: u32, axis: Axis) -> Result<()> {
    let mass_b = vector(g,b+48)?;
    let pa = mul(c,splat(gain),vector(g,a+48)?)?;
    let pb = mul(c,splat(gain),mass_b)?;
    let linear = matches!(axis,Axis::Linear|Axis::LinearUnbounded);
    if linear {
        let normal = vector(g,row)?;
        let da = mul(c,splat((pa>>96) as u32),normal)?;
        let db = mul(c,splat((pb>>96) as u32),normal)?;
        let new_a = add(c,vector(g,a+16)?,da)?;
        put_vector(g,a+16,new_a)?;
        let new_b = sub(c,vector(g,b+16)?,db)?;
        put_vector(g,b+16,new_b)?;
        if axis == Axis::LinearUnbounded {
            set(c,3,da);
            set(c,4,db);
        } else {
            set(c,3,db);
            set(c,4,new_a);
        }
    } else if axis == Axis::Angular {
        set(c,3,vector(g,row+16)?);
        set(c,4,mass_b);
    }
    let angular_row = if linear {row+16} else {row};
    let da = mul(c,pa,vector(g,angular_row)?)?;
    let db = mul(c,pb,vector(g,angular_row+16)?)?;
    let new_a = add(c,vector(g,a+32)?,da)?;
    put_vector(g,a+32,new_a)?;
    let new_b = add(c,vector(g,b+32)?,db)?;
    put_vector(g,b+32,new_b)?;
    set(c,0,new_b);
    set(c,1,db);
    set(c,2,new_a);
    x.fadd_m(g.u32(impulse)?).map_err(fp_error)?;
    g.set_u32(impulse,x.fstp_m32().map_err(fp_error)?)
}

pub(super) fn compare(c: &mut CpuAbi, x: &mut X87, bits: u32, pop: bool, mask: u8) -> Result<u8> {
    let code = x87::abi::cmp(&mut x.0,0,x87::abi::src_kind::F32,u64::from(bits),u32::from(pop),false);
    X87::ok(code).map_err(fp_error)?;
    c.eax = (c.eax & !0xffff) | u32::from(x.0.status);
    let flags = (x.0.status>>8) as u8 & mask;
    logic(c,u32::from(flags),128);
    Ok(flags)
}

pub(super) fn residual(g: &Guest, x: &mut X87, rhs: u32, dot: u32, scale: u32, velocity_scale: u32) -> Result<()> {
    x.fld(g.u32(rhs)?).map_err(fp_error)?;
    x.fmul_m(g.u32(scale)?).map_err(fp_error)?;
    x.fld(dot).map_err(fp_error)?;
    x.fmul_m(g.u32(velocity_scale)?).map_err(fp_error)?;
    x.arith_reg(op::SUB,1,0,true).map_err(fp_error)
}

pub(super) fn single(c: &mut CpuAbi, g: &mut Guest, form: u32) -> Result<Option<TransferAbi>> {
    use Axis::*;
    let (axis,spill,size,mut last) = match form {
        0x31d982 => (Angular,76,16,0x31da41),
        0x31df3e => (Linear,116,16,0x31e046),
        0x31dc3f => (Linear,100,8,0x31dd6c),
        0x31d796 => (AngularCompact,128,8,0x31d87a),
        0x31d88e => (Angular,68,16,0x31d963),
        0x31dd80 => (Linear,108,12,0x31df1f),
        0x31da55 => (Angular,84,4,0x31db0e),
        0x31db21 => (LinearUnbounded,92,4,0x31dc2c),
        _ => return Ok(Some(TransferAbi::resume(0x31c9b4))),
    };
    let (a,b,sp,row,cmd,imp) = (c.ecx,g.u32(c.esp+12)?,c.esp,c.esi,c.ebx,c.edi);
    let params = g.u32(c.ebp+8)?;
    let angular = matches!(axis,Angular|AngularCompact);
    let rhs = row + if angular {28} else {12};
    let inv = row + if angular {12} else {28};
    let bounded = matches!(form,0x31d982|0x31df3e);
    let capped = matches!(form,0x31dc3f|0x31d796);
    let ranged = matches!(form,0x31d88e|0x31dd80);
    if capped || size==4 || form==0x31d982 {g.set_u32(sp+16,b+48)?;}
    if size==4 || form==0x31dc3f || form==0x31dd80 {g.set_u32(sp+20,b+32)?;}
    if axis==LinearUnbounded {g.set_u32(sp+32,b+16)?;}
    if capped {
        g.set_u32(sp+24,params+12)?;
        g.set_u32(sp+if angular {60} else {56},g.u32(cmd+4)?)?;
    }
    let projection = project(c,g,a,b,row,angular)?;
    let dot = projection.xyz as u32;
    g.set_u32(sp+spill,dot)?;
    set(c,0,projection.lanes);
    set(c,1,projection.xy);
    if angular && axis!=AngularCompact {
        set(c,2,vector(g,row)?);
        set(c,3,vector(g,row+16)?);
        set(c,4,projection.xyz);
    } else {
        set(c,2,projection.xyz);
        if !angular {set(c,3,projection.angular_b);set(c,4,vector(g,row)?);}
    }
    let mut x = load_fpu(c);
    let mut apply_last = true;
    if bounded {
        x.fld(g.u32(inv)?).map_err(fp_error)?;
        x.fld(g.u32(rhs)?).map_err(fp_error)?;
        x.arith_m(op::SUB,dot).map_err(fp_error)?;
        x.arith_st(op::MUL,1).map_err(fp_error)?;
        x.fmul_m(g.u32(params+4)?).map_err(fp_error)?;
        x.fstp_st(1).map_err(fp_error)?;
        let bound = if compare(c,&mut x,g.u32(cmd+4)?,false,65)?==0 {Some(cmd+4)}
            else if compare(c,&mut x,g.u32(cmd+8)?,false,5)?.count_ones()==1 {Some(cmd+8)} else {None};
        if let Some(bound) = bound {
            x.fld(g.u32(bound)?).map_err(fp_error)?;
            x.arith_st(op::DIV,1).map_err(fp_error)?;
            x.arith_reg(op::MUL,1,0,true).map_err(fp_error)?;
        }
    } else if ranged {
        x.fld(dot).map_err(fp_error)?;
        x.fmul_m(g.u32(params+8)?).map_err(fp_error)?;
        let scale = if angular {cmd+12} else {params+4};
        x.fld(g.u32(rhs)?).map_err(fp_error)?;
        x.arith_m(op::SUB,g.u32(cmd+4)?).map_err(fp_error)?;
        x.fmul_m(g.u32(scale)?).map_err(fp_error)?;
        x.arith_st(op::SUB,1).map_err(fp_error)?;
        if angular {g.set_u32(sp+28,x.fst_m32().map_err(fp_error)?)?;}
        let positive = compare(c,&mut x,g.u32(ZERO)?,angular,65)?==0;
        if angular && positive {
            x.fstp_st(0).map_err(fp_error)?;
            x.fld(g.u32(sp+28)?).map_err(fp_error)?;
        } else {
            if !angular {
                if positive {
                    x.fmul_m(g.u32(inv)?).map_err(fp_error)?;
                    let factor = x.fst_m32().map_err(fp_error)?;
                    g.set_u32(sp+28,factor)?;
                    apply(c,g,&mut x,a,b,row,imp,factor,axis)?;
                } else {x.fstp_st(0).map_err(fp_error)?;}
            }
            x.fld(g.u32(rhs)?).map_err(fp_error)?;
            x.arith_m(op::SUB,g.u32(cmd+8)?).map_err(fp_error)?;
            x.fmul_m(g.u32(scale)?).map_err(fp_error)?;
            x.arith_reg(op::SUBR,1,0,true).map_err(fp_error)?;
            apply_last = compare(c,&mut x,g.u32(ZERO)?,false,5)?.count_ones()==1;
        }
        if apply_last {x.fmul_m(g.u32(inv)?).map_err(fp_error)?;}
    } else if capped {
        residual(g,&mut x,rhs,dot,params+12,params+8)?;
        x.fmul_m(g.u32(inv)?).map_err(fp_error)?;
        x.fld_st(0).map_err(fp_error)?;
        x.unary(unary_op::ABS).map_err(fp_error)?;
        let limit = g.u32(sp+if angular {60} else {56})?;
        if compare(c,&mut x,limit,false,65)?==0 {
            x.arith_m(op::DIVR,limit).map_err(fp_error)?;
            x.fld_st(0).map_err(fp_error)?;
            x.arith_reg(op::MUL,2,0,true).map_err(fp_error)?;
            x.fmul_m(g.u32(rhs)?).map_err(fp_error)?;
            g.set_u32(rhs,x.fstp_m32().map_err(fp_error)?)?;
        } else {x.fstp_st(0).map_err(fp_error)?;}
    } else {
        x.fld(g.u32(inv)?).map_err(fp_error)?;
        residual(g,&mut x,rhs,dot,params+4,params+8)?;
        x.arith_reg(op::MUL,1,0,true).map_err(fp_error)?;
    }
    let operand = if apply_last {
        let factor = x.fst_m32().map_err(fp_error)?;
        g.set_u32(sp+28,factor)?;
        apply(c,g,&mut x,a,b,row,imp,factor,axis)?;
        imp
    } else {
        x.fstp_st(0).map_err(fp_error)?;
        last = if angular {0x31d967} else {0x31df23};
        ZERO
    };
    finish_fpu(c,g,x,last,operand)?;
    if form==0x31df3e {
        c.eax=a;
        c.edx=params;
        c.ecx=row;
        return Ok(Some(call_arg(c,g,1,0x31c530,0x31e04d)?));
    }
    c.ebx+=size;
    c.esi+=if angular {32} else {48};
    c.edi+=4;
    Ok(None)
}

fn subtract_velocity(c: &mut CpuAbi, g: &mut Guest, dot: u32, scratch: u32, rhs: u32, store_pc: u32) -> Result<()> {
    g.set_u32(c.esp+scratch,dot)?;
    let mut x = load_fpu(c);
    x.fld(g.u32(rhs)?).map_err(fp_error)?;
    x.arith_m(op::SUB,dot).map_err(fp_error)?;
    g.set_u32(rhs,x.fstp_m32().map_err(fp_error)?)?;
    finish_fpu(c,g,x,store_pc,rhs)
}

pub(super) fn correct_velocities(c: &mut CpuAbi, g: &mut Guest) -> Result<Option<u32>> {
    let first = g.u32(c.esp+44)?==0;
    c.eax=g.u32(c.ebp+16)?;
    loop {
        c.ecx=byte(c.ecx,g.u8(c.eax)?);
        match c.ecx as u8 {
            3 => break,
            2 => for (from,to) in [(32,80),(16,64)] {
                let v=vector(g,c.eax+from)?;
                set(c,0,v);
                let v=if first {v} else {
                    let sum=add(c,vector(g,c.eax+to)?,v)?;
                    set(c,1,sum);
                    sum
                };
                put_vector(g,c.eax+to,v)?;
            },
            _ => {
                // The guest spins on an unrecognized accumulator type.
                c.edx=(c.ecx as i8 as i32 as u32).wrapping_sub(3);
                let cf=(c.ecx as i8 as i32 as u32)<2;
                arithmetic(c,c.edx.wrapping_add(1),1,true);
                c.eflags=(c.eflags & !1)|u32::from(cf);
                return Ok(Some(if first {0x31e0d0} else {0x31e124}));
            }
        }
        c.eax+=128;
    }
    c.ecx=g.u32(c.ebp+12)?;
    c.edx=g.u32(c.ebp+20)?;
    let mut header=true;
    let mut second_pair=false;
    loop {
        if header {
            c.esi=g.u32(c.ecx+12)?;
            c.edi=g.u32(c.ecx+16)?;
            c.eax=g.u32(c.ecx+4)?;
            c.ecx+=24;
            header=false;
        }
        c.ebx=g.u8(c.ecx+2)? as i8 as i32 as u32;
        arithmetic(c,c.ebx,6,true);
        if c.ebx>6 {return Ok(Some(0x31e0f0));}
        let form=g.u32(CORRECTION_DISPATCH+c.ebx*4)?;
        let (a,b,row)=(c.esi,c.edi,c.eax);
        match form {
            0x31e4da => return Ok(None),
            0x31e0e1 => {header=true;second_pair=false;continue;}
            0x31e400 | 0x31e472 => {
                let angular=form==0x31e472;
                let v=project(c,g,a,b,row,angular)?;
                set(c,0,v.lanes);set(c,1,v.xy);set(c,2,v.xyz);
                if !angular {set(c,3,v.angular_b);}
                let (slot,rhs,pc)=if angular {(188,row+28,0x31e4bb)} else {(180,row+12,0x31e45e)};
                subtract_velocity(c,g,v.xyz as u32,slot,rhs,pc)?;
                c.eax+=if angular {32} else {48};
                c.edx+=4;
            }
            0x31e161 | 0x31e2e9 => {
                let triple=form==0x31e2e9;
                let v=project(c,g,a,b,row,false)?;
                let w=project(c,g,a,b,row+48,false)?;
                set(c,0,v.lanes);set(c,1,w.xyz);set(c,2,v.xy);set(c,3,v.xyz);
                set(c,4,vector(g,row+48)?);set(c,5,vector(g,row)?);set(c,6,v.linear);
                let (slot,p0,p1)=if triple {(156,0x31e38d,0x31e3a5)}
                    else if second_pair {(140,0x31e2c5,0x31e2d6)} else {(124,0x31e1fd,0x31e20e)};
                subtract_velocity(c,g,v.xyz as u32,slot,row+12,p0)?;
                subtract_velocity(c,g,w.xyz as u32,slot+8,row+60,p1)?;
                if triple {
                    let v=project(c,g,a,b,row+96,true)?;
                    set(c,0,v.lanes);set(c,1,v.xy);set(c,2,v.xyz);
                    subtract_velocity(c,g,v.xyz as u32,172,row+124,0x31e3e5)?;
                }
                c.eax+=if triple {128} else {96};
                c.edx+=if triple {12} else {8};
            }
            _ => {}
        }
        second_pair=form==0x31e161 && !second_pair;
        c.ecx+=u32::from(g.u8(c.ecx+1)?);
        if form==0x31e2e9 && g.u8(c.ecx)?==1 {header=true;}
    }
}
