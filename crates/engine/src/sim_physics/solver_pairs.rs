//! Two coupled contact normals, optionally with a third angular row.
//! The two unrolled unilateral paths round at different stack locations.
use super::*;
use super::havok::vector;
use super::solver_rows::{Axis, ZERO, apply, compare, project, residual, set};
use super::spatial::{finish_fpu, fp_error, load_fpu};
use crate::light_constants::X87;
use x87::abi::{arith_op as op, unary_op};

pub(super) fn coupled(c: &mut CpuAbi, g: &mut Guest, form: u32, alternate: bool) -> Result<()> {
    let (row,sp,a,b)=(c.esi,c.esp,c.ecx,g.u32(c.esp+12)?);
    let one_sided=form==0x31c9c6;
    let triple=form==0x31d1fb;
    let (first,second)=if triple {(64,168)} else if !one_sided {(120,184)}
        else if alternate {(136,88)} else {(104,112)};
    let v=project(c,g,a,b,row,false)?;
    let w=project(c,g,a,b,row+48,false)?;
    g.set_u32(sp+first,v.xyz as u32)?;
    g.set_u32(sp+second,w.xyz as u32)?;
    if triple {g.set_u32(sp+32,b+16)?;}
    else if alternate || !one_sided {g.set_u32(sp+20,b+32)?;}
    set(c,0,w.lanes);
    set(c,1,w.xy);
    set(c,2,w.xyz);
    set(c,3,vector(g,row+48)?);
    set(c,4,vector(g,row)?);
    let params=g.u32(c.ebp+8)?;
    let mut x=load_fpu(c);
    if triple {
        let v=project(c,g,a,b,row+96,true)?;
        g.set_u32(sp+144,v.xyz as u32)?;
        x.fld(g.u32(row+108)?).map_err(fp_error)?;
        residual(g,&mut x,row+124,v.xyz as u32,params+12,params+8)?;
        x.arith_st(op::MUL,1).map_err(fp_error)?;
        g.set_u32(sp+20,x.fstp_m32().map_err(fp_error)?)?;
        x.fstp_st(0).map_err(fp_error)?;
    }
    let scale=params+if one_sided {4} else {12};
    let temp=if triple || alternate {16} else if one_sided {20} else {24};
    residual(g,&mut x,row+12,v.xyz as u32,scale,params+8)?;
    g.set_u32(sp+temp,x.fstp_m32().map_err(fp_error)?)?;
    residual(g,&mut x,row+60,w.xyz as u32,scale,params+8)?;
    let (last,addr)=if one_sided {
        unilateral(c,g,&mut x,alternate,temp)?
    } else {
        limited(c,g,&mut x,triple,temp)?
    };
    finish_fpu(c,g,x,last,addr)?;
    c.ebx+=if triple {24} else if one_sided {8} else {20};
    c.esi+=if triple {128} else {96};
    c.edi+=if triple {12} else {8};
    Ok(())
}

fn unilateral(c: &mut CpuAbi, g: &mut Guest, x: &mut X87, alternate: bool, temp: u32) -> Result<(u32,u32)> {
    let (row,sp,cmd,imp,a,b)=(c.esi,c.esp,c.ebx,c.edi,c.ecx,g.u32(c.esp+12)?);
    let r0=g.u32(sp+temp)?;
    let cross=g.u32(cmd+4)?;
    x.fld(g.u32(row+44)?).map_err(fp_error)?;
    x.fmul_m(r0).map_err(fp_error)?;
    x.fld_st(1).map_err(fp_error)?;
    x.fmul_m(cross).map_err(fp_error)?;
    x.arith_reg(op::ADD,1,0,true).map_err(fp_error)?;
    let d0=x.fstp_m32().map_err(fp_error)?;
    g.set_u32(sp+32,d0)?;
    x.fld(g.u32(row+92)?).map_err(fp_error)?;
    if alternate {x.arith_st(op::MUL,1).map_err(fp_error)?;}
    x.fld(r0).map_err(fp_error)?;
    x.fmul_m(cross).map_err(fp_error)?;
    if !alternate {
        x.fxch(1).map_err(fp_error)?;
        x.arith_st(op::MUL,2).map_err(fp_error)?;
    }
    x.arith_reg(op::ADD,1,0,true).map_err(fp_error)?;
    let d1=x.fstp_m32().map_err(fp_error)?;
    g.set_u32(sp+if alternate {24} else {16},d1)?;
    x.fld(d0).map_err(fp_error)?;
    let zero=g.u32(ZERO)?;
    if compare(c,x,zero,true,65)?==0 {
        x.fstp_st(0).map_err(fp_error)?;
        x.fld(d1).map_err(fp_error)?;
        if compare(c,x,zero,true,65)?==0 {
            x.fld(d0).map_err(fp_error)?;
            g.set_u32(sp+if alternate {176} else {72},d0)?;
            apply(c,g,x,a,b,row,imp,d0,Axis::Linear)?;
            x.fld(d1).map_err(fp_error)?;
            if alternate {g.set_u32(sp+16,b+48)?;g.set_u32(sp+32,row+48)?;}
            g.set_u32(sp+if alternate {96} else {160},d1)?;
            apply(c,g,x,a,b,row+48,imp+4,d1,Axis::Linear)?;
            return Ok((if alternate {0x31cf8f} else {0x31cbf5},imp+4));
        }
    } else {
        x.fmul_m(g.u32(row+76)?).map_err(fp_error)?;
        if compare(c,x,zero,false,65)?.count_ones().is_multiple_of(2) {
            let gain=x.fst_m32().map_err(fp_error)?;
            g.set_u32(sp+24,gain)?;
            if alternate {g.set_u32(sp+32,row+48)?;}
            apply(c,g,x,a,b,row+48,imp+4,gain,Axis::Linear)?;
            return Ok((if alternate {0x31d1e0} else {0x31cd34},imp+4));
        }
        x.fstp_st(0).map_err(fp_error)?;
    }
    x.fld(r0).map_err(fp_error)?;
    x.fmul_m(g.u32(row+28)?).map_err(fp_error)?;
    if compare(c,x,zero,false,65)?==0 {
        let gain=x.fst_m32().map_err(fp_error)?;
        g.set_u32(sp+24,gain)?;
        apply(c,g,x,a,b,row,imp,gain,Axis::Linear)?;
        Ok((if alternate {0x31d153} else {0x31ccaa},imp))
    } else {
        x.fstp_st(0).map_err(fp_error)?;
        Ok((if alternate {0x31d1e5} else {0x31cd39},ZERO))
    }
}

fn limited(c: &mut CpuAbi, g: &mut Guest, x: &mut X87, triple: bool, temp: u32) -> Result<(u32,u32)> {
    let (row,sp,cmd,imp,a,b)=(c.esi,c.esp,c.ebx,c.edi,c.ecx,g.u32(c.esp+12)?);
    let r0=g.u32(sp+temp)?;
    let r1=x.fstp_m32().map_err(fp_error)?;
    g.set_u32(sp+if triple {24} else {32},r1)?;
    let limit=g.u32(cmd+12)?;
    if triple {g.set_u32(sp+52,limit)?;} else {
        x.fld(limit).map_err(fp_error)?;
        x.fld_st(0).map_err(fp_error)?;
        x.arith_st(op::MUL,1).map_err(fp_error)?;
        g.set_u32(sp+16,x.fstp_m32().map_err(fp_error)?)?;
        x.fstp_st(0).map_err(fp_error)?;
    }
    let cross=g.u32(cmd+8)?;
    for (diag,r,s) in [(row+44,r0,r1),(row+92,r1,r0)] {
        if triple {
            x.fld(r).map_err(fp_error)?;
            x.fmul_m(g.u32(diag)?).map_err(fp_error)?;
            x.fld(s).map_err(fp_error)?;
            x.fmul_m(cross).map_err(fp_error)?;
        } else {
            x.fld(g.u32(diag)?).map_err(fp_error)?;
            x.fld(s).map_err(fp_error)?;
            x.fmul_m(cross).map_err(fp_error)?;
            x.fxch(1).map_err(fp_error)?;
            x.fmul_m(r).map_err(fp_error)?;
        }
        x.arith_reg(op::ADD,1,0,true).map_err(fp_error)?;
    }
    let clipped=if triple {
        x.fld(g.u32(sp+20)?).map_err(fp_error)?;
        x.fld_st(0).map_err(fp_error)?;
        x.arith_st(op::MUL,1).map_err(fp_error)?;
        for i in [2,3] {
            x.fld_st(i).map_err(fp_error)?;
            x.arith_st(op::MUL,i+1).map_err(fp_error)?;
            x.arith_reg(op::ADD,1,0,true).map_err(fp_error)?;
        }
        x.fld(limit).map_err(fp_error)?;
        x.fmul_m(limit).map_err(fp_error)?;
        x.fld_st(1).map_err(fp_error)?;
        let rc=x87::abi::cmp(&mut x.0,0,x87::abi::src_kind::ST,1,2,false);
        X87::ok(rc).map_err(fp_error)?;
        x.0.status & 0x4100==0
    } else {
        for _ in 0..2 {
            x.fld_st(1).map_err(fp_error)?;
            x.arith_st(op::MUL,2).map_err(fp_error)?;
        }
        x.arith_reg(op::ADD,1,0,true).map_err(fp_error)?;
        compare(c,x,g.u32(sp+16)?,false,65)?==0
    };
    if clipped {
        if triple {
            x.unary(unary_op::SQRT).map_err(fp_error)?;
            x.arith_m(op::DIVR,limit).map_err(fp_error)?;
        } else {
            x.arith_m(op::DIVR,g.u32(sp+16)?).map_err(fp_error)?;
            x.unary(unary_op::SQRT).map_err(fp_error)?;
        }
        let n=if triple {4} else {3};
        for i in [n,n-1] {
            x.fld_st(0).map_err(fp_error)?;
            x.arith_reg(op::MUL,i,0,true).map_err(fp_error)?;
        }
        if triple {
            x.fxch(1).map_err(fp_error)?;
            x.arith_st(op::MUL,1).map_err(fp_error)?;
            x.fxch(1).map_err(fp_error)?;
        }
        let count=if triple {3} else {2};
        for i in 0..count {
            if i+1<count {x.fld_st(0).map_err(fp_error)?;}
            let rhs=row+[12,60,124][i];
            x.fmul_m(g.u32(rhs)?).map_err(fp_error)?;
            g.set_u32(rhs,x.fstp_m32().map_err(fp_error)?)?;
        }
    } else {x.fstp_st(0).map_err(fp_error)?;}
    if triple {
        x.fmul_m(g.u32(cmd+20)?).map_err(fp_error)?;
        g.set_u32(sp+20,x.fstp_m32().map_err(fp_error)?)?;
    }
    x.fld_st(1).map_err(fp_error)?;
    let d0=x.fstp_m32().map_err(fp_error)?;
    g.set_u32(sp+if triple {24} else {28},d0)?;
    x.fxch(1).map_err(fp_error)?;
    apply(c,g,x,a,b,row,imp,d0,Axis::Linear)?;
    g.set_u32(sp+16,b+48)?;
    g.set_u32(sp+24,row+48)?;
    let d1=x.fst_m32().map_err(fp_error)?;
    g.set_u32(sp+28,d1)?;
    apply(c,g,x,a,b,row+48,imp+4,d1,Axis::Linear)?;
    if triple {
        let twist=g.u32(sp+20)?;
        x.fld(twist).map_err(fp_error)?;
        g.set_u32(sp+152,twist)?;
        apply(c,g,x,a,b,row+96,imp+8,twist,Axis::AngularCompact)?;
        Ok((0x31d51d,imp+8))
    } else {Ok((0x31d780,imp+4))}
}
