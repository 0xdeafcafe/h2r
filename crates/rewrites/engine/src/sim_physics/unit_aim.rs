//! Transform the unit's look directions into joint space, enforce yaw and
//! pitch limits and derive the direction and angular velocity of the joint.
use super::*;
use super::spatial::{finish_fpu,fp_error,load_fpu,scalar};
use crate::light_constants::X87;
use x87::abi::{arith_op,unary_op};

const TAU:u32=0x0043_f17c;
const ONE:u32=0x0045_dbc0;
const NEG_ONE:u32=0x0045_dbcc;
const SMALL:u32=0x0045_dbdc;

/// Where [`step`] leaves the function: `call target` returning to `site`
/// (the return address not pushed yet), or `ret pop` (not popped yet).
pub(crate) enum Exit { Call(u32,u32), Ret(u16) }
pub(crate) const SITES:&[u32]=&[0xc7b4d,0xc7e2d,0xc7e43,0xc7e51,0xc7e78,0xc7ea1,0xc80a3,0xc80e0,0xc81cb];

/// Scalar XMM assignments distinguish a fresh load (clears upper lanes)
/// from a register arithmetic result (retains its upper lanes).
fn low(c:&CpuAbi,r:usize)->u32 {u128::from_le_bytes(c.xmm[r]) as u32}
fn mov(c:&mut CpuAbi,d:usize,r:usize) {c.xmm[d]=c.xmm[r];}
type Op=fn(&mut u32,u128,u32)->std::result::Result<u128,simd::Xm>;
fn bin(c:&mut CpuAbi,r:usize,b:u32,op:Op)->Result<()> {
    let a=u128::from_le_bytes(c.xmm[r]);
    c.xmm[r]=op(&mut c.mxcsr,a,b).map_err(super::havok::simd_fault)?.to_le_bytes();
    Ok(())
}
fn mul(c:&mut CpuAbi,r:usize,b:u32)->Result<()> {bin(c,r,b,simd::mulss)}
fn add(c:&mut CpuAbi,r:usize,b:u32)->Result<()> {bin(c,r,b,simd::addss)}
fn sub(c:&mut CpuAbi,r:usize,b:u32)->Result<()> {bin(c,r,b,simd::subss)}
fn div(c:&mut CpuAbi,r:usize,b:u32)->Result<()> {bin(c,r,b,simd::divss)}
fn above(c:&mut CpuAbi,a:u32,b:u32)->Result<bool> {
    let f=simd::comiss(&mut c.mxcsr,a,b).map_err(super::havok::simd_fault)?;
    c.eflags=c.eflags & !ARITH | f;c.eflags_defined|=ARITH;
    Ok(f & 0x41==0)
}
fn ld(c:&mut CpuAbi,g:&Guest,r:usize,p:u32)->Result<()> {scalar(c,r,g.u32(p)?);Ok(())}
fn st(c:&CpuAbi,g:&mut Guest,r:usize,p:u32)->Result<()> {g.set_u32(p,low(c,r))}
fn fstore(x:&mut X87,g:&mut Guest,p:u32)->Result<()> {g.set_u32(p,x.fstp_m32().map_err(fp_error)?)}
fn square(x:&mut X87,g:&Guest,p:u32)->Result<()> {
    let v=g.u32(p)?;x.fld(v).map_err(fp_error)?;x.fmul_m(v).map_err(fp_error)
}
fn angles(x:&mut X87,g:&mut Guest,src:u32,out:u32)->Result<()> {
    x.fld(g.u32(src+4)?).map_err(fp_error)?;x.fld(g.u32(src)?).map_err(fp_error)?;
    x.unary(unary_op::PATAN).map_err(fp_error)?;fstore(x,g,out)?;
    x.fld(g.u32(src+8)?).map_err(fp_error)?;
    square(x,g,src+4)?;square(x,g,src)?;
    x.arith_reg(arith_op::ADD,1,0,true).map_err(fp_error)?;
    x.unary(unary_op::SQRT).map_err(fp_error)?;
    x.fxch(1).map_err(fp_error)?;x.fxch(1).map_err(fp_error)?;
    x.unary(unary_op::PATAN).map_err(fp_error)?;
    fstore(x,g,out+4)
}
fn direction(x:&mut X87,g:&mut Guest,ang:u32,out:u32)->Result<()> {
    x.fld(g.u32(ang+4)?).map_err(fp_error)?;x.fcos().map_err(fp_error)?;
    x.fld(g.u32(ang)?).map_err(fp_error)?;x.fcos().map_err(fp_error)?;
    x.arith_st(arith_op::MUL,1).map_err(fp_error)?;fstore(x,g,out)?;
    x.fld(g.u32(ang)?).map_err(fp_error)?;x.fsin().map_err(fp_error)?;
    x.arith_st(arith_op::MUL,1).map_err(fp_error)?;fstore(x,g,out+4)?;
    x.fstp_st(0).map_err(fp_error)?;
    x.fld(g.u32(ang+4)?).map_err(fp_error)?;x.fsin().map_err(fp_error)?;
    fstore(x,g,out+8)
}

fn transform_inputs(c:&mut CpuAbi,g:&mut Guest)->Result<()> {
    let sp=c.esp;
    if c.esi==0 {
        for (src,off) in [(c.edi,12),(c.eax,52)] {
            for k in [0,4,8] {let v=g.u32(src+k)?;g.set_u32(sp+off+k,v)?;}
        }
        c.edx=g.u32(c.eax+8)?;
        return Ok(());
    }
    for (second,src,out) in [(false,c.edi,sp+12),(true,c.eax,sp+52)] {
        let m=c.esi;
        let [x,y,z]=[g.u32(src)?,g.u32(src+4)?,g.u32(src+8)?];
        scalar(c,3,g.u32(m+4)?);mul(c,3,x)?;
        scalar(c,4,g.u32(m+12)?);mul(c,4,z)?;add(c,3,low(c,4))?;
        scalar(c,4,g.u32(m+8)?);mul(c,4,y)?;add(c,3,low(c,4))?;st(c,g,3,out)?;
        if second {
            scalar(c,3,x);mul(c,3,g.u32(m+16)?)?;
            scalar(c,4,g.u32(m+24)?);mul(c,4,z)?;add(c,3,low(c,4))?;
            scalar(c,4,g.u32(m+20)?);mul(c,4,y)?;
        } else {
            scalar(c,3,g.u32(m+20)?);mul(c,3,y)?;
            scalar(c,4,x);mul(c,4,g.u32(m+16)?)?;add(c,3,low(c,4))?;
            scalar(c,4,g.u32(m+24)?);mul(c,4,z)?;
        }
        add(c,3,low(c,4))?;st(c,g,3,out+4)?;
        scalar(c,3,g.u32(m+28)?);mul(c,3,x)?;
        scalar(c,0,g.u32(m+36)?);mul(c,0,z)?;add(c,3,low(c,0))?;
        scalar(c,0,g.u32(m+32)?);mul(c,0,y)?;add(c,3,low(c,0))?;st(c,g,3,out+8)?;
        scalar(c,1,y);scalar(c,2,z);
    }
    Ok(())
}

// Joint-to-world multiply. Both sequences use Z,Y,X for the sums.
// The first site leaves different scratch lanes live across normalize().
fn transform_out(c:&mut CpuAbi,g:&mut Guest,dest:u32,first:bool)->Result<()> {
    let m=c.esi;
    let src=c.esp+56;
    let [x,y,z]=[g.u32(src)?,g.u32(src+4)?,g.u32(src+8)?];
    scalar(c,0,x);scalar(c,1,if first {y} else {z});scalar(c,2,if first {z} else {y});
    for i in [0,4,8] {
        scalar(c,3,g.u32(m+28+i)?);mul(c,3,z)?;
        let r=if i==8 {if first {2} else {1}} else {4};
        scalar(c,r,g.u32(m+16+i)?);mul(c,r,y)?;add(c,3,low(c,r))?;
        let r=if i==8 {1} else {4};
        scalar(c,r,g.u32(m+4+i)?);mul(c,r,x)?;add(c,3,low(c,r))?;
        st(c,g,3,dest+i)?;
    }
    c.eax=dest;
    Ok(())
}

fn begin(c:&mut CpuAbi,g:&mut Guest)->Result<Exit> {
    c.esp=c.esp.wrapping_sub(128);
    transform_inputs(c,g)?;
    c.ecx=g.u32(c.esp+140)?;
    ld(c,g,0,c.ecx+4)?;sub(c,0,g.u32(c.ecx)?)?;
    ld(c,g,1,TAU)?;sub(c,0,low(c,1))?;
    let wrap=above(c,low(c,0),g.u32(0x45ddfc)?)?;
    push(c,g,c.ebx)?;
    let sp=c.esp;
    c.ebx=byte(c.ebx,u8::from(wrap));
    g.set_u8(sp+7,u8::from(wrap))?;
    let mut x=load_fpu(c);
    angles(&mut x,g,sp+16,sp+36)?;
    angles(&mut x,g,sp+56,sp+28)?;
    c.edx=byte(c.edx,1);
    ld(c,g,2,c.ecx)?;ld(c,g,0,sp+28)?;
    let lo=low(c,2);let hi=g.u32(c.ecx+4)?;
    if above(c,lo,low(c,0))? {
        if wrap {add(c,0,low(c,1))?;} else {scalar(c,0,lo);c.edx &= !255;}
        st(c,g,0,sp+28)?;
    } else if above(c,low(c,0),hi)? {
        if wrap {sub(c,0,low(c,1))?;} else {scalar(c,0,hi);c.edx &= !255;}
        st(c,g,0,sp+28)?;
    }
    ld(c,g,2,c.ecx+8)?;ld(c,g,0,sp+32)?;
    let mut rebuild=c.edx as u8==0;
    if above(c,low(c,2),low(c,0))? {mov(c,0,2);st(c,g,0,sp+32)?;rebuild=true;}
    else if above(c,low(c,0),g.u32(c.ecx+12)?)? {ld(c,g,0,c.ecx+12)?;st(c,g,0,sp+32)?;rebuild=true;}
    if rebuild {
        logic(c,c.esi,1<<31);
        direction(&mut x,g,sp+28,sp+56)?;
        finish_fpu(c,g,x,0xc7aad,sp+64)?;
        if c.esi!=0 {
            transform_out(c,g,sp+56,true)?;
            return Ok(Exit::Call(0x30bf0,0xc7b4d));
        }
    } else {
        c.ecx=g.u32(c.eax)?;c.edx=g.u32(c.eax+4)?;c.eax=g.u32(c.eax+8)?;
        for (off,v) in [(56,c.ecx),(60,c.edx),(64,c.eax)] {g.set_u32(sp+off,v)?;}
        finish_fpu(c,g,x,0xc7a2d,sp+32)?;
    }
    velocity(c,g)
}

fn velocity(c:&mut CpuAbi,g:&mut Guest)->Result<Exit> {
    let sp=c.esp;
    let p=g.u32(sp+140)?;
    c.edx=g.u32(p)?;c.eax=g.u32(p+4)?;c.ecx=g.u32(p+8)?;
    g.set_u32(sp+44,c.edx)?;g.set_u32(sp+52,c.ecx)?;g.set_u32(sp+48,c.eax)?;
    let mut x=load_fpu(c);
    square(&mut x,g,sp+44)?;square(&mut x,g,sp+52)?;
    x.arith_reg(arith_op::ADD,1,0,true).map_err(fp_error)?;
    square(&mut x,g,sp+48)?;x.arith_reg(arith_op::ADD,1,0,true).map_err(fp_error)?;
    x.unary(unary_op::SQRT).map_err(fp_error)?;
    g.set_u32(sp+8,x.fst_m32().map_err(fp_error)?)?;
    x.unary(unary_op::ABS).map_err(fp_error)?;
    x.fld(g.u32(SMALL)?).map_err(fp_error)?;
    let flags=x.fcomip(1).map_err(fp_error)?;
    x.fstp_st(0).map_err(fp_error)?;
    ld(c,g,3,sp+40)?;ld(c,g,4,sp+36)?;scalar(c,7,0);
    let mut rotate=false;
    if flags & 0x41!=0 {
        ld(c,g,5,sp+8)?;ld(c,g,2,ONE)?;ld(c,g,0,sp+48)?;ld(c,g,1,sp+52)?;
        let magnitude=low(c,5);
        let cmp=simd::ucomiss(&mut c.mxcsr,magnitude,0).map_err(super::havok::simd_fault)?;
        div(c,2,low(c,5))?;mov(c,6,2);mul(c,6,g.u32(sp+44)?)?;mul(c,0,low(c,2))?;mul(c,1,low(c,2))?;
        rotate=cmp & 0x44!=0x40;
        if !rotate {ld(c,g,1,TAU)?;}
    }
    if rotate {
        rotate_velocity(c,g,&mut x)?;
        finish_fpu(c,g,x,0xc7d5f,sp+12)?;
    } else {
        finish_fpu(c,g,x,0xc7bab,SMALL)?;
        st(c,g,7,sp+16)?;st(c,g,7,sp+20)?;
    }
    logic(c,c.ebx&255,128);
    sub(c,4,g.u32(sp+28)?)?;sub(c,3,g.u32(sp+32)?)?;
    st(c,g,4,sp+8)?;st(c,g,3,sp+12)?;
    if c.ebx as u8!=0 {
        if above(c,low(c,4),g.u32(0x45dddc)?)? {sub(c,4,low(c,1))?;st(c,g,4,sp+8)?;}
        else {
            ld(c,g,0,0x45df50)?;
            if above(c,low(c,0),low(c,4))? {add(c,4,low(c,1))?;st(c,g,4,sp+8)?;}
        }
    }
    c.eax=g.u32(sp+16)?;c.ecx=g.u32(sp+8)?;push(c,g,c.ebp)?;
    c.ebp=g.u32(sp+148)?;c.edx=sp+68;push(c,g,c.edx)?;
    c.edx=g.u32(sp+152)?;
    for v in [c.edx,c.ebp,c.eax,c.ecx] {push(c,g,v)?;}
    Ok(Exit::Call(0xc7300,0xc7e2d))
}

fn rotate_velocity(c:&mut CpuAbi,g:&mut Guest,x:&mut X87)->Result<()> {
    let sp=c.esp;
    x.fld(g.u32(sp+8)?).map_err(fp_error)?;
    x.fmul_m(g.u32(sp+136)?).map_err(fp_error)?;x.fld_st(0).map_err(fp_error)?;x.fsin().map_err(fp_error)?;
    c.edx=g.u32(sp+16)?;c.ecx=g.u32(sp+24)?;c.eax=g.u32(sp+20)?;
    g.set_u32(sp+52,c.ecx)?;ld(c,g,4,sp+52)?;
    g.set_u32(sp+44,c.edx)?;ld(c,g,3,sp+44)?;
    mov(c,2,3);mul(c,2,low(c,6))?;mov(c,5,4);mul(c,5,low(c,1))?;add(c,2,low(c,5))?;
    g.set_u32(sp+48,c.eax)?;ld(c,g,7,sp+48)?;
    mov(c,5,7);mul(c,5,low(c,0))?;add(c,2,low(c,5))?;
    ld(c,g,5,ONE)?;mul(c,7,low(c,6))?;mul(c,4,low(c,6))?;
    fstore(x,g,sp+16)?;x.fcos().map_err(fp_error)?;fstore(x,g,sp+8)?;
    sub(c,5,g.u32(sp+8)?)?;mul(c,2,low(c,5))?;
    mov(c,5,1);mul(c,5,low(c,3))?;sub(c,4,low(c,5))?;
    mov(c,5,0);mul(c,5,low(c,3))?;mul(c,3,g.u32(sp+8)?)?;sub(c,5,low(c,7))?;
    mov(c,7,2);mul(c,7,low(c,6))?;ld(c,g,6,sp+52)?;add(c,3,low(c,7))?;
    ld(c,g,7,sp+48)?;mul(c,6,low(c,0))?;mul(c,7,low(c,1))?;sub(c,7,low(c,6))?;
    ld(c,g,6,sp+16)?;mul(c,7,low(c,6))?;sub(c,3,low(c,7))?;
    ld(c,g,7,sp+8)?;st(c,g,3,sp+44)?;ld(c,g,3,sp+48)?;
    mul(c,0,low(c,2))?;mul(c,3,low(c,7))?;add(c,0,low(c,3))?;
    mul(c,4,low(c,6))?;sub(c,0,low(c,4))?;st(c,g,0,sp+48)?;
    x.fld(g.u32(sp+48)?).map_err(fp_error)?;x.fld(g.u32(sp+44)?).map_err(fp_error)?;
    x.unary(unary_op::PATAN).map_err(fp_error)?;
    ld(c,g,0,sp+52)?;mul(c,1,low(c,2))?;mul(c,0,low(c,7))?;add(c,1,low(c,0))?;
    mul(c,5,low(c,6))?;sub(c,1,low(c,5))?;st(c,g,1,sp+52)?;fstore(x,g,sp+8)?;
    x.fld(g.u32(sp+52)?).map_err(fp_error)?;square(x,g,sp+44)?;square(x,g,sp+48)?;
    x.arith_reg(arith_op::ADD,1,0,true).map_err(fp_error)?;x.unary(unary_op::SQRT).map_err(fp_error)?;
    x.fxch(1).map_err(fp_error)?;x.fxch(1).map_err(fp_error)?;x.unary(unary_op::PATAN).map_err(fp_error)?;
    ld(c,g,0,ONE)?;div(c,0,g.u32(sp+136)?)?;
    ld(c,g,1,sp+8)?;ld(c,g,4,sp+36)?;ld(c,g,3,sp+40)?;
    sub(c,1,low(c,4))?;mul(c,1,low(c,0))?;st(c,g,1,sp+16)?;
    fstore(x,g,sp+12)?;ld(c,g,1,sp+12)?;
    sub(c,1,low(c,3))?;mul(c,1,low(c,0))?;st(c,g,1,sp+20)?;
    ld(c,g,1,TAU)
}

/// The port: the transfer, made on the guest stack.
pub(super) fn aim(c:&mut CpuAbi,g:&mut Guest,pc:u32)->Result<TransferAbi> {
    match step(c,g,pc)? {
        Exit::Call(target,site)=>g.call(c,target,site),
        Exit::Ret(pop)=>g.ret(c,u32::from(pop)),
    }
}

/// From the entry or a site to the next transfer, without making it:
/// [`aim`] pushes the return address or pops it, and the object update's
/// body (`crate::sim_objects::unit`) leaves that to its dispatcher.
pub(crate) fn step(c:&mut CpuAbi,g:&mut Guest,pc:u32)->Result<Exit> {
    let sp=c.esp;
    match pc {
        0xc7b4d=>{ld(c,g,1,TAU)?;velocity(c,g)}
        0xc7e2d=>{
            c.ecx=g.u32(sp+24)?;c.eax=sp+104;push(c,g,c.eax)?;c.eax=g.u32(sp+16)?;
            for v in [c.edx,c.ebp,c.ecx,c.eax] {push(c,g,v)?;}
            Ok(Exit::Call(0xc7300,0xc7e43))
        }
        0xc7e43=>{push(c,g,c.edx)?;c.ecx=sp+104;c.eax=sp+72;Ok(Exit::Call(0xc75d0,0xc7e51))}
        0xc7e51 | 0xc7e78=>{
            let first=pc==0xc7e51;
            let off=if first {12} else {16};
            ld(c,g,4,sp+off+8)?;ld(c,g,5,sp+off)?;ld(c,g,3,sp+140)?;
            push(c,g,sp+off)?;
            c.edx=sp+off+28;c.ecx=sp+if first {72} else {104};
            if !first {c.ebx=byte(c.ebx,c.eax as u8);}
            Ok(Exit::Call(0xc7750,if first {0xc7e78} else {0xc7ea1}))
        }
        0xc7ea1=>{
            let arrived=c.ebx as u8!=0 && c.eax as u8!=0;
            c.ebp=pop(c,g)?;
            if !arrived {return steer(c,g);}
            let sp=c.esp;
            c.eax=g.u32(sp+56)?;c.ecx=g.u32(sp+60)?;c.edx=g.u32(sp+64)?;
            g.set_u32(c.edi,c.eax)?;c.eax=g.u32(sp+140)?;
            g.set_u32(c.edi+4,c.ecx)?;c.ecx=g.u32(0x4687a4)?;g.set_u32(c.edi+8,c.edx)?;
            c.edx=g.u32(c.ecx)?;g.set_u32(c.eax,c.edx)?;
            c.edx=g.u32(c.ecx+4)?;g.set_u32(c.eax+4,c.edx)?;
            c.ecx=g.u32(c.ecx+8)?;g.set_u32(c.eax+8,c.ecx)?;
            leave(c,g)
        }
        0xc80a3=>{
            ld(c,g,1,NEG_ONE)?;ld(c,g,0,sp+8)?;
            if above(c,low(c,1),low(c,0))? {st(c,g,1,sp+8)?;}
            else {ld(c,g,1,ONE)?;let hi=above(c,low(c,0),low(c,1))?;st(c,g,if hi{1}else{0},sp+8)?;}
            let mut x=load_fpu(c);x.fld(g.u32(sp+8)?).map_err(fp_error)?;
            finish_fpu(c,g,x,0xc80d7,sp+8)?;
            Ok(Exit::Call(0x322bd8,0xc80e0))
        }
        0xc80e0=>apply_speed(c,g),
        0xc81cb=>leave(c,g),
        _=>begin(c,g),
    }
}
fn leave(c:&mut CpuAbi,g:&Guest)->Result<Exit> {
    c.ebx=pop(c,g)?;c.esp=arithmetic(c,c.esp,128,false);Ok(Exit::Ret(20))
}

fn steer(c:&mut CpuAbi,g:&mut Guest)->Result<Exit> {
    let sp=c.esp;
    c.eax=byte(c.eax,g.u8(sp+7)?);
    ld(c,g,0,sp+8)?;ld(c,g,1,sp+12)?;add(c,0,g.u32(sp+28)?)?;add(c,1,g.u32(sp+32)?)?;
    let wrap=c.eax as u8!=0;
    c.eax=g.u32(sp+144)?;ld(c,g,2,c.eax)?;
    st(c,g,0,sp+16)?;st(c,g,1,sp+20)?;
    if above(c,low(c,2),low(c,0))? {
        if wrap {add(c,0,g.u32(TAU)?)?;} else {ld(c,g,0,c.eax)?;}
        st(c,g,0,sp+16)?;
    } else if above(c,low(c,0),g.u32(c.eax+4)?)? {
        if wrap {sub(c,0,g.u32(TAU)?)?;} else {c.edx=g.u32(sp+144)?;ld(c,g,0,c.edx+4)?;}
        st(c,g,0,sp+16)?;
    }
    if !wrap {c.eax=g.u32(sp+144)?;}
    ld(c,g,0,c.eax+8)?;
    if above(c,low(c,0),low(c,1))? {st(c,g,0,sp+20)?;}
    else if above(c,low(c,1),g.u32(c.eax+12)?)? {ld(c,g,0,c.eax+12)?;st(c,g,0,sp+20)?;}
    let mut x=load_fpu(c);
    direction(&mut x,g,sp+16,sp+56)?;
    ld(c,g,3,sp+56)?;ld(c,g,5,sp+60)?;ld(c,g,4,sp+64)?;
    for (delta,angle) in [(36,16),(40,20)] {
        x.fld(g.u32(sp+delta)?).map_err(fp_error)?;x.fmul_m(g.u32(sp+136)?).map_err(fp_error)?;
        x.fadd_m(g.u32(sp+angle)?).map_err(fp_error)?;
    }
    x.fld_st(0).map_err(fp_error)?;x.fcos().map_err(fp_error)?;
    x.fld_st(2).map_err(fp_error)?;x.fcos().map_err(fp_error)?;
    x.arith_st(arith_op::MUL,1).map_err(fp_error)?;fstore(&mut x,g,sp+44)?;
    ld(c,g,6,sp+44)?;mov(c,0,6);mul(c,0,low(c,3))?;
    x.fxch(2).map_err(fp_error)?;x.fsin().map_err(fp_error)?;
    x.arith_st(arith_op::MUL,2).map_err(fp_error)?;fstore(&mut x,g,sp+48)?;
    x.fstp_st(1).map_err(fp_error)?;x.fsin().map_err(fp_error)?;fstore(&mut x,g,sp+52)?;
    finish_fpu(c,g,x,0xc8011,sp+52)?;
    ld(c,g,2,sp+52)?;mov(c,1,2);mul(c,1,low(c,4))?;add(c,0,low(c,1))?;
    ld(c,g,1,sp+48)?;mov(c,7,1);mul(c,7,low(c,5))?;add(c,0,low(c,7))?;
    ld(c,g,7,NEG_ONE)?;
    let lower=above(c,low(c,7),low(c,0))?;st(c,g,0,sp+8)?;
    if lower {st(c,g,7,sp+8)?;} else {
        ld(c,g,7,ONE)?;
        if above(c,low(c,0),low(c,7))? {st(c,g,7,sp+8)?;}
    }
    c.ebx=g.u32(sp+140)?;
    mov(c,7,1);mov(c,0,2);mul(c,7,low(c,4))?;mul(c,0,low(c,5))?;
    mul(c,4,low(c,6))?;mul(c,2,low(c,3))?;mul(c,1,low(c,3))?;mul(c,5,low(c,6))?;
    sub(c,0,low(c,7))?;sub(c,4,low(c,2))?;sub(c,1,low(c,5))?;
    c.eax=c.ebx;
    for (i,r) in [0,4,1].into_iter().enumerate() {st(c,g,r,c.ebx+4*i as u32)?;}
    Ok(Exit::Call(0x30bf0,0xc80a3))
}
fn apply_speed(c:&mut CpuAbi,g:&mut Guest)->Result<Exit> {
    let sp=c.esp;
    let mut x=load_fpu(c);
    x.arith_m(arith_op::DIV,g.u32(sp+136)?).map_err(fp_error)?;
    ld(c,g,0,sp+148)?;fstore(&mut x,g,sp+8)?;
    x.fld(g.u32(sp+148)?).map_err(fp_error)?;x.fld(g.u32(sp+8)?).map_err(fp_error)?;
    let comparison=x.fcomip(1).map_err(fp_error)?;x.fstp_st(0).map_err(fp_error)?;
    finish_fpu(c,g,x,0xc8101,sp+8)?;
    if comparison & 0x41!=0 {ld(c,g,0,sp+8)?;}
    logic(c,c.esi,1<<31);
    for i in [0,4,8] {ld(c,g,1,c.ebx+i)?;mul(c,1,low(c,0))?;st(c,g,1,c.ebx+i)?;}
    if c.esi!=0 {transform_out(c,g,c.edi,false)?;return Ok(Exit::Call(0x30bf0,0xc81cb));}
    c.eax=g.u32(sp+56)?;c.ecx=g.u32(sp+60)?;c.edx=g.u32(sp+64)?;
    g.set_u32(c.edi,c.eax)?;g.set_u32(c.edi+4,c.ecx)?;g.set_u32(c.edi+8,c.edx)?;
    leave(c,g)
}
