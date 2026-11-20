//! Fetch the cached support simplex from both shapes and transform the
//! second shape's packed support vertices into the first shape's space.
use super::*;
use super::havok::{put_vector, simd_fault, splat, vector};

pub(super) const SITES: &[u32] = &[0x3048e0,0x3048f8,0x3049f0,0x304a41,0x304ab4,0x304afa,0x304c25,0x304c57,
    0x304e21,0x305285,0x305401,0x30550f];
pub(super) const REST: &[u32] = &[];
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
    if pc!=SITES[1] {
        let stage=match pc {
            0x3049f0=>{
                c.eax=arith_dec(c,c.eax);
                if c.eax==0 {Direction(true)} else {c.eax=arith_dec(c,c.eax);if c.eax==0 {edge_pair(c,g)?} else {Direction(false)}}
            }
            0x304a41=>pair_answer(c,g)?,
            0x304ab4|0x304c25=>tetra_answer(c,g,pc==0x304ab4)?,
            0x304afa|0x304c57=>Triangle(pc==0x304afa),
            0x304e21|0x305285|0x305401=>candidate(c,g,pc==0x305285,pc==0x305401)?,
            _=>{g.set_u32(c.esp+32,c.eax)?;Output}
        };
        return advance(c,g,stage);
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
    advance(c,g,Iteration)
}

use super::spatial::{finish_fpu,fp_error,load_fpu};
use crate::light_constants::X87;
use x87::abi::{arith_op,unary_op};

type Vec4=u128;
fn reg(c:&CpuAbi,n:usize)->Vec4 {u128::from_le_bytes(c.xmm[n])}
fn set(c:&mut CpuAbi,n:usize,v:Vec4)->Vec4 {c.xmm[n]=v.to_le_bytes();v}
fn shuffle(a:Vec4,b:Vec4,mask:u8)->Vec4 {
    let mut out=0;
    for i in 0..4 {
        let source=if i<2 {a} else {b};
        let lane=(source>>(32*((mask>>(i*2))&3))) as u32;
        out|=u128::from(lane)<<(32*i);
    }
    out
}
fn mask(v:Vec4)->u32 {
    (0..4).fold(0,|out,i| out|(((v>>(31+i*32)) as u32&1)<<i))
}
fn prod(c:&mut CpuAbi,a:Vec4,b:Vec4)->Result<Vec4>{simd::mulps(&mut c.mxcsr,a,b).map_err(simd_fault)}
fn plus(c:&mut CpuAbi,a:Vec4,b:Vec4)->Result<Vec4>{simd::addps(&mut c.mxcsr,a,b).map_err(simd_fault)}
fn minus(c:&mut CpuAbi,a:Vec4,b:Vec4)->Result<Vec4>{simd::subps(&mut c.mxcsr,a,b).map_err(simd_fault)}
fn sum3(c:&mut CpuAbi,v:Vec4,mid:usize,end:usize)->Result<Vec4> {
    let xy=simd::addss(&mut c.mxcsr,splat((v>>32) as u32),v as u32).map_err(simd_fault)?;
    let xyz=simd::addss(&mut c.mxcsr,splat((v>>64) as u32),xy as u32).map_err(simd_fault)?;
    set(c,mid,xy);set(c,end,xyz);
    Ok(xyz)
}
fn move_vertex(c:&mut CpuAbi,g:&mut Guest,from:u32,to:u32)->Result<()> {
    let v=vector(g,from)?;set(c,0,v);put_vector(g,to,v)
}
fn shifted(c:&mut CpuAbi,v:u32,n:u32)->u32 {
    let y=v<<n;logic(c,y,1<<31);
    c.eflags|=(v>>(32-n))&1;c.eflags_defined &= !0x800;y
}
fn inc(c:&mut CpuAbi,v:u32)->u32 {
    let cf=c.eflags&1;let valid=c.eflags_defined&1;
    let result=arithmetic(c,v,1,false);
    c.eflags=c.eflags&!1|cf;c.eflags_defined=c.eflags_defined&!1|valid;result
}

enum Step {Iteration,Classify,Edge(bool),Triangle(bool),Direction(bool),TryB,TryLastA,Output,Finish,Call(TransferAbi)}
use Step::*;

fn advance(c:&mut CpuAbi,g:&mut Guest,mut stage:Step)->Result<TransferAbi> {
    let sp=c.esp;
    loop {
        stage=match stage {
            Call(t)=>return Ok(t),
            Iteration=>{
                let mut x=load_fpu(c);
                x.fld(g.u32(sp+20)?).map_err(fp_error)?;
                x.arith_st(arith_op::ADD,0).map_err(fp_error)?;
                g.set_u32(sp+20,x.fstp_m32().map_err(fp_error)?)?;
                finish_fpu(c,g,x,0x304983,sp+20)?;
                c.edx=g.u32(sp+28)?.wrapping_add(1);
                g.set_u32(sp+28,c.edx)?;g.set_u32(c.esi+16,0)?;Classify
            }
            Classify=>classify(c,g)?,
            Edge(b)=>edge(c,g,b)?,
            Triangle(b)=>triangle(c,g,b)?,
            Direction(degenerate)=>direction(c,g,degenerate)?,
            TryB=>try_b(c,g)?,
            TryLastA=>{
                c.eax=g.u32(sp+76)?;
                if c.eax==0 {request_a(c,g,true)?} else {Output}
            }
            Finish=>finish_simplex(c,g)?,
            Output=>return output(c,g),
        };
    }
}

fn classify(c:&mut CpuAbi,g:&mut Guest)->Result<Step> {
    c.eax=g.u32(c.esi)?;c.ebx=g.u32(c.edi)?;
    c.eax=((c.eax<<3)|c.ebx).wrapping_sub(9);
    arithmetic(c,c.eax,24,true);
    let target=if c.eax<=24 {
        c.ecx=u32::from(g.u8(0x30567c+c.eax)?);g.u32(0x305650+4*c.ecx)?
    } else {0x304d40};
    match target {
        0x3051c3=>point_pair(c,g),
        0x304fd6|0x3050e5=>Ok(Edge(target==0x304fd6)),
        0x304ae6|0x304c43=>triangle_call(c,g,target==0x304ae6),
        0x304aa1|0x304c12=>{
            let b=target==0x304aa1;
            let sign=g.u32(target+1)?;push(c,g,sign)?;
            c.eax=c.esi+if b{160}else{32};c.ebx=c.esi+if b{32}else{160};
            Ok(Call(g.call(c,0x303540,if b{0x304ab4}else{0x304c25})?))
        }
        0x3049fe=>edge_pair(c,g),
        0x3049ae|0x3049cc=>{
            let reverse=target==0x3049cc;
            c.edx=g.u32(c.esi+8)?;
            push(c,g,if reverse {u32::MAX} else {1})?;push(c,g,c.ecx)?;
            c.eax=c.esp;
            let eq=arithmetic(c,c.edx,2,true)==0;
            push(c,g,if reverse{c.esi}else{c.edi})?;
            if !reverse {c.edx=byte(c.edx,u8::from(eq));}
            g.set_u8(c.eax,u8::from(eq))?;
            push(c,g,if reverse{c.edi}else{c.esi})?;
            push(c,g,c.esi+if reverse{32}else{160})?;
            c.eax=c.esi+if reverse{160}else{32};c.ecx=c.esi;
            Ok(Call(call_arg(c,g,c.eax,0x3037a0,0x3049f0)?))
        }
        _=>Ok(Direction(true)),
    }
}
fn point_pair(c:&mut CpuAbi,g:&mut Guest)->Result<Step> {
    let first=vector(g,c.esi+32)?;
    let delta=minus(c,vector(g,c.esi+160)?,first)?;
    set(c,0,first);set(c,1,delta);put_vector(g,c.esi+288,delta)?;
    Ok(Direction(false))
}
fn triangle_call(c:&mut CpuAbi,g:&mut Guest,b:bool)->Result<Step> {
    push(c,g,if b {1}else{u32::MAX})?;
    c.eax=c.esi+if b{160}else{32};push(c,g,c.eax)?;
    c.ebx=c.esi+if b{32}else{160};push(c,g,c.ebx)?;c.ecx=c.esi;
    Ok(Call(g.call(c,0x303360,if b{0x304afa}else{0x304c57})?))
}
fn edge_pair(c:&mut CpuAbi,g:&mut Guest)->Result<Step> {
    let sp=c.esp;
    let a=c.esi+32;let b=c.esi+160;
    let da=minus(c,vector(g,a+16)?,vector(g,a)?)?;
    c.eax=b;c.ebx=a;
    for v in [c.esi+304,sp+80,b] {push(c,g,v)?;}
    c.edx=sp+96;put_vector(g,sp+96,da)?;
    let origin=vector(g,b)?;set(c,0,origin);
    let db=minus(c,vector(g,b+16)?,origin)?;set(c,1,db);
    push(c,g,c.edx)?;push(c,g,c.ebx)?;put_vector(g,sp+80,db)?;
    c.ecx=sp+80;
    Ok(Call(g.call(c,0x308590,0x304a41)?))
}
fn pair_answer(c:&mut CpuAbi,g:&mut Guest)->Result<Step> {
    c.esp=c.esp.wrapping_add(20);
    if c.eax!=0 {
        if c.eax&1!=0 {move_vertex(c,g,c.esi+48,c.ebx)?;}
        if c.eax&3!=0 {g.set_u32(c.esi,1)?;}
        if c.eax&4!=0 {
            move_vertex(c,g,c.esi+176,c.esi+160)?;move_vertex(c,g,c.esi+240,c.esi+224)?;
        }
        if c.eax&12!=0 {g.set_u32(c.edi,1)?;}
        return Ok(Classify);
    }
    let v=vector(g,c.esp+80)?;let w=vector(g,c.esp+96)?;
    let vw=prod(c,shuffle(w,w,210),shuffle(v,v,201))?;
    let wv=prod(c,shuffle(w,w,201),shuffle(v,v,210))?;
    let cross=minus(c,wv,vw)?;
    let delta=minus(c,vector(g,c.ebx)?,vector(g,c.esi+160)?)?;
    let dot=prod(c,cross,delta)?;set(c,0,dot);set(c,1,cross);
    let s=sum3(c,dot,2,3)?;c.eax=mask(s);
    put_vector(g,c.esi+288,cross)?;
    if c.eax&1==0 {
        let signs=vector(g,0x4124a0)?;set(c,0,signs);set(c,1,cross^signs);
        put_vector(g,c.esi+288,cross^signs)?;
    }
    Ok(Direction(false))
}

fn tetra_answer(c:&mut CpuAbi,g:&mut Guest,b:bool)->Result<Step> {
    c.esp=c.esp.wrapping_add(4);
    if (c.eax as i32)<0 {return Ok(Direction(true));}
    let i=c.eax;
    if b {
        c.ebx=g.u32(c.edi)?.wrapping_sub(1);g.set_u32(c.edi,c.ebx)?;
        c.ecx=(i+10)<<4;
        move_vertex(c,g,c.esi+208,c.esi.wrapping_add(c.ecx))?;
        c.eax=shifted(c,i+14,4);
        move_vertex(c,g,c.esi+272,c.esi.wrapping_add(c.eax))?;
    } else {
        c.edx=g.u32(c.esi)?.wrapping_sub(1);g.set_u32(c.esi,c.edx)?;
        c.eax=shifted(c,i+2,4);
        move_vertex(c,g,c.esi+80,c.esi.wrapping_add(c.eax))?;
    }
    triangle_call(c,g,b)
}

fn triangle(c:&mut CpuAbi,g:&mut Guest,b:bool)->Result<Step> {
    if c.eax==7 {return Ok(Direction(false));}
    let feature=g.u8(0x4136a8u32.wrapping_add(c.eax))? as i8 as i32;
    let base=c.esi+if b{160}else{32};
    let count=if b{c.edi}else{c.esi};
    if feature<0 {
        let mut slot=feature+8;
        if slot>3 {slot=1;}
        let off=(slot as u32).wrapping_mul(16);
        let n=g.u32(count)?.wrapping_sub(1);
        g.set_u32(count,n)?;
        if b {c.ebx=n;} else {c.edx=n;}
        move_vertex(c,g,base+32,base.wrapping_add(off))?;
        if b {move_vertex(c,g,base+96,base.wrapping_add(off).wrapping_add(64))?;}
        return Ok(Edge(b));
    }
    let f=feature as u32;
    let i=g.u8(0x4136b0+f)? as i8 as i32 as u32;
    let j=g.u8(0x4136b2+f)? as i8 as i32 as u32;
    let at=|n:u32|base.wrapping_add(n.wrapping_mul(16));
    c.ecx=j;
    if b {c.edx=i;g.set_u32(c.esp+16,at(j))?;} else {c.edx=at(j);g.set_u32(c.esp+16,i)?;}
    c.eax=at(i);
    let origin=vector(g,at(f))?;
    let d=minus(c,vector(g,c.ebx)?,origin)?;
    let du=minus(c,vector(g,at(i))?,origin)?;
    let dv=minus(c,vector(g,at(j))?,origin)?;
    set(c,1,d);set(c,2,dv);
    let dot=prod(c,d,du)?;
    let val=sum3(c,dot,3,4)?;
    let last=vector(g,base+32)?;set(c,0,last);
    c.ebx=mask(val);g.set_u32(count,2)?;
    if c.ebx&1==0 {
        if b {c.edx=g.u32(c.esp+16)?;}
        put_vector(g,at(j),last)?;
        if b {c.ecx=(j+14)<<4;move_vertex(c,g,base+96,at(j)+64)?;}
        return Ok(Classify);
    }
    put_vector(g,at(i),last)?;
    if b {
        c.eax=(i+14)<<4;
        move_vertex(c,g,base+96,at(i)+64)?;
    }
    let dot=prod(c,d,dv)?;set(c,1,dot);
    let sign=mask(sum3(c,dot,0,2)?);
    if b {c.eax=sign;} else {c.edx=sign;}
    if sign&1!=0 {
        if j==g.u32(count)? {c.ecx=i;}
        let n=g.u32(count)?.wrapping_sub(1);g.set_u32(count,n)?;
        c.eax=(n+if b{10}else{2})<<4;
        move_vertex(c,g,at(n),at(c.ecx))?;
        if b {c.edx=(c.ecx+10)<<4;c.eax=(n+14)<<4;move_vertex(c,g,at(n)+64,at(c.ecx)+64)?;}
        c.ecx=(c.ecx+if b{14}else{2})<<4;
    }
    Ok(Classify)
}

fn edge(c:&mut CpuAbi,g:&mut Guest,b:bool)->Result<Step> {
    let base=c.esi+if b{160}else{32};
    let opposing=c.esi+if b{32}else{160};
    let count=if b{c.edi}else{c.esi};
    let last=vector(g,base+16)?;set(c,5,last);
    let first=vector(g,base)?;let other=vector(g,opposing)?;
    let edge=minus(c,last,first)?;
    let d0=minus(c,first,other)?;let d1=minus(c,last,other)?;
    set(c,2,edge);set(c,3,d1);set(c,4,d0);
    let p1=prod(c,d1,edge)?;let s1=mask(sum3(c,p1,6,7)?)&1;
    let p0=prod(c,d0,edge)?;set(c,1,p0);
    let s0=mask(sum3(c,p0,0,6)?)&1;
    c.eax=s1;if b {c.edx=s0;} else {c.ecx=s0;}
    if s1==s0 {
        if s1!=0 {
            put_vector(g,base,last)?;
            move_vertex(c,g,base+80,base+64)?;
        }
        let n=g.u32(count)?.wrapping_sub(1);g.set_u32(count,n)?;
        return point_pair(c,g);
    }
    let t1=prod(c,shuffle(d1,d1,210),shuffle(d0,d0,201))?;
    let rot=shuffle(d1,d1,201);set(c,4,rot);
    let t0=prod(c,rot,shuffle(d0,d0,210))?;set(c,3,t0);
    let cross=minus(c,t0,t1)?;set(c,0,cross);
    let (u,v)=if b {(edge,cross)} else {(cross,edge)};
    let t0=prod(c,shuffle(v,v,210),shuffle(u,u,201))?;
    let r=shuffle(u,u,210);set(c,1,r);
    let t1=prod(c,shuffle(v,v,201),r)?;
    set(c,3,t0);
    let axis=minus(c,t1,t0)?;set(c,if b{2}else{0},axis);
    put_vector(g,c.esi+288,axis)?;
    Ok(Direction(false))
}

fn direction(c:&mut CpuAbi,g:&mut Guest,degenerate:bool)->Result<Step> {
    let sp=c.esp;
    if !degenerate {
        c.edx=g.u32(c.edi)?;
        g.set_u32(c.esi+8,c.edx)?;
    }
    c.eax=u32::from(degenerate);logic(c,c.eax,1<<31);
    c.ebx=c.esi+288;
    let axis=vector(g,c.ebx)?;
    let squared=prod(c,axis,axis)?;
    let norm=sum3(c,squared,1,2)?;
    c.ecx=sp+36;g.set_u32(c.ecx,norm as u32)?;
    let delta=minus(c,vector(g,c.esi+160)?,vector(g,c.esi+32)?)?;
    let p=prod(c,delta,axis)?;set(c,0,p);
    let dot=sum3(c,p,1,2)?;
    c.edx=sp+24;g.set_u32(c.edx,dot as u32)?;
    if degenerate {return Ok(Finish);}
    let mut x=load_fpu(c);
    x.fld(dot as u32).map_err(fp_error)?;
    x.fmul_m(dot as u32).map_err(fp_error)?;x.fmul_m(dot as u32).map_err(fp_error)?;
    x.fld(g.u32(sp+20)?).map_err(fp_error)?;
    x.fmul_m(g.u32(0x41359c)?).map_err(fp_error)?;
    x.fmul_m(g.u32(sp+36)?).map_err(fp_error)?;
    X87::ok(x87::abi::cmp(&mut x.0,0,x87::abi::src_kind::ST,1,2,false)).map_err(fp_error)?;
    c.eax=(c.eax & !65535)|u32::from(x.0.status);
    let carry=(c.eax>>8)&1;logic(c,carry,128);
    if carry==0 {finish_fpu(c,g,x,0x304dca,sp+36)?;return Ok(Finish);}
    x.fld(g.u32(sp+36)?).map_err(fp_error)?;
    x.fmul_m(g.u32(sp+20)?).map_err(fp_error)?;
    c.eax=g.u32(sp+28)? & 1;
    g.set_u32(sp+76,c.eax)?;
    g.set_u32(sp+16,x.fstp_m32().map_err(fp_error)?)?;
    finish_fpu(c,g,x,0x304dea,sp+16)?;
    if c.eax==0 {Ok(TryB)} else {request_a(c,g,false)}
}
fn request_a(c:&mut CpuAbi,g:&mut Guest,last:bool)->Result<Step> {
    c.ecx=g.u32(c.ebp+20)?;
    c.edx=g.u8(c.ecx+10)? as i8 as i32 as u32;
    c.eax=g.u32(c.esi)?;
    if c.eax==c.edx || g.u32(c.esi+16)?==1 {return Ok(if last {Output} else {TryB});}
    c.ecx=g.u32(c.ebp+8)?;c.edx=g.u32(c.ecx)?;
    c.eax=arithmetic(c,c.eax.wrapping_add(2)<<4,c.esi,false);
    push(c,g,c.eax)?;push(c,g,c.ebx)?;
    let target=g.u32(c.edx+48)?;
    Ok(Call(g.call(c,target,if last{0x305401}else{0x304e21})?))
}

fn try_b(c:&mut CpuAbi,g:&mut Guest)->Result<Step> {
    c.eax=g.u32(c.ebp+20)?;c.ecx=g.u8(c.eax+11)? as i8 as i32 as u32;c.edx=g.u32(c.edi)?;
    if c.edx==c.ecx || g.u32(c.esi+16)?==u32::MAX {return Ok(TryLastA);}
    let m=g.u32(c.ebp+16)?;
    let [x,y,z]=[vector(g,m)?,vector(g,m+16)?,vector(g,m+32)?];
    c.ecx=g.u32(c.ebp+12)?;c.eax=g.u32(c.ecx)?;
    let axis=vector(g,c.ebx)?;let signs=vector(g,0x4124a0)?;set(c,5,signs);
    let lo=shuffle(x,y,68);let hi=shuffle(x,y,238);
    let zl=shuffle(z,z,68);set(c,4,zl);
    let zh=shuffle(z,z,238);set(c,7,zh);
    let depth=prod(c,shuffle(hi,zh,136),splat((axis>>64) as u32))?;set(c,2,depth);
    let height=prod(c,shuffle(lo,zl,221),splat((axis>>32) as u32))?;set(c,6,height);
    let ax=splat(axis as u32);set(c,3,ax);set(c,1,axis);
    let width=prod(c,shuffle(lo,zl,136),ax)?;
    let xy=plus(c,width,height)?;let local=plus(c,xy,depth)?^signs;set(c,0,local);
    c.edx=arithmetic(c,c.edx.wrapping_add(14)<<4,c.esi,false);
    push(c,g,c.edx)?;c.edx=c.esp+116;push(c,g,c.edx)?;put_vector(g,c.edx,local)?;
    let target=g.u32(c.eax+48)?;
    Ok(Call(g.call(c,target,0x305285)?))
}

// Reject newly returned supports that fail to improve the separating
// distance. For two and three old vertices, the second test uses C0/C2
// parity (unordered is treated differently from the first test).
fn candidate(c:&mut CpuAbi,g:&mut Guest,b:bool,last:bool)->Result<Step> {
    let sp=c.esp;let base=c.esi+if b{224}else{32};
    let slot=if b{c.edi}else{c.esi};
    c.ecx=g.u32(slot)?;
    let ptr=base.wrapping_add(c.ecx.wrapping_mul(16));
    let axis=if b {vector(g,sp+112)?} else {vector(g,c.ebx)?};
    let vertex=vector(g,ptr)?;
    set(c,if b{1}else{0},vertex);set(c,2,axis);
    let reject=if b {TryLastA} else if last {Output} else {TryB};
    let offset=if b{52}else if last{64}else{40};
    for (i,prior) in [0,2,1].into_iter().enumerate() {
        if i==1 && c.ecx!=3 || i==2 && !(2..=3).contains(&c.ecx) {continue;}
        let old=vector(g,base+16*prior)?;
        let delta=minus(c,vertex,old)?;
        let dot=prod(c,delta,axis)?;
        let (value,mid,end)=if i==2 {
            if b{(1,0,2)}else{(0,1,2)}
        } else if b{(0,3,4)}else{(1,3,4)};
        set(c,value,dot);
        let v=sum3(c,dot,mid,end)? as u32;
        let at=sp+offset+4*i as u32;
        g.set_u32(at,v)?;
        if b {c.edx=ptr;c.eax=at;} else if i==1 {c.eax=at;} else {c.edx=at;}
        let mut x=load_fpu(c);
        x.fld(v).map_err(fp_error)?;x.unary(unary_op::ABS).map_err(fp_error)?;
        x.fmul_m(v).map_err(fp_error)?;
        X87::ok(x87::abi::cmp(&mut x.0,0,x87::abi::src_kind::F32,u64::from(g.u32(sp+16)?),1,false)).map_err(fp_error)?;
        c.eax=(c.eax & !65535)|u32::from(x.0.status);
        let f=(c.eax>>8)&if i==0{65}else{5};
        logic(c,f,128);
        let pc=if b{[0x3052d4,0x30532c,0x305370][i]}else if last{[0x305448,0x30549d,0x3054de][i]}else{[0x304e68,0x304ebd,0x304efe][i]};
        finish_fpu(c,g,x,pc,sp+16)?;
        if (i==0 && f!=0)||(i!=0 && f.count_ones()%2!=0) {return Ok(reject);}
        if i==0 {
            c.eax=arithmetic(c,c.ecx,2,true);
            if c.eax!=0 {c.eax=arith_dec(c,c.eax);}
        }
    }
    if b {
        let m=g.u32(c.ebp+16)?;
        let v=vector(g,c.edx)?;
        let z=prod(c,vector(g,m+32)?,splat((v>>64) as u32))?;
        let zt=plus(c,z,vector(g,m+48)?)?;set(c,3,zt);
        let y=prod(c,vector(g,m+16)?,splat((v>>32) as u32))?;set(c,2,y);
        let sx=splat(v as u32);set(c,1,sx);
        let x=prod(c,vector(g,m)?,sx)?;let xy=plus(c,x,y)?;let out=plus(c,xy,zt)?;
        set(c,0,out);c.eax=m;
        c.ecx=shifted(c,c.ecx.wrapping_add(10),4);
        put_vector(g,c.esi.wrapping_add(c.ecx),out)?;
        let count=inc(c,g.u32(c.edi)?);g.set_u32(c.edi,count)?;
    } else {c.ecx=inc(c,c.ecx);g.set_u32(c.esi,c.ecx)?;}
    Ok(Iteration)
}
fn arith_dec(c:&mut CpuAbi,v:u32)->u32 {
    let cf=c.eflags&1;let valid=c.eflags_defined&1;let result=arithmetic(c,v,1,true);
    c.eflags=c.eflags&!1 | cf;c.eflags_defined=c.eflags_defined&!1|valid;result
}
fn finish_simplex(c:&mut CpuAbi,g:&mut Guest)->Result<Step> {
    c.eax=byte(c.eax,g.u8(c.esi+12)?);logic(c,c.eax&255,128);
    if c.eax as u8==0 {
        c.eax=g.u32(c.ebp+20)?;c.ecx=g.u32(c.ebp+16)?;c.edx=g.u32(c.ebp+12)?;
        push(c,g,c.eax)?;c.eax=g.u32(c.ebp+8)?;
        for v in [c.ecx,c.edx,c.eax] {push(c,g,v)?;}
        c.ecx=c.esi;return Ok(Call(g.call(c,0x303dd0,0x30550f)?));
    }
    c.eax=g.u32(c.edi)?;c.ecx=g.u32(c.esi)?;c.edx=c.eax.wrapping_add(c.ecx);
    if (c.edx as i32)>4 {
        let first=(c.ecx as i32)>(c.eax as i32);g.set_u32(c.esi,if first{3}else{1})?;
        g.set_u32(c.edi,if first{1}else{3})?;
    }
    g.set_u32(c.esp+32,3)?;
    Ok(Output)
}
fn output(c:&mut CpuAbi,g:&mut Guest)->Result<TransferAbi> {
    let sp=c.esp;
    let delta=minus(c,vector(g,c.esi+160)?,vector(g,c.esi+32)?)?;
    let n=vector(g,c.ebx)?;c.eax=g.u32(c.ebp+24)?;
    let nn=prod(c,n,n)?;
    let squared=sum3(c,nn,2,1)?;
    put_vector(g,sp+112,squared)?;
    let approx=simd::rsqrtss(reg(c,2),squared as u32) as u32;
    g.set_u32(sp+112,approx)?;
    let once=simd::mulss(&mut c.mxcsr,squared,approx).map_err(simd_fault)?;
    let twice=simd::mulss(&mut c.mxcsr,once,approx).map_err(simd_fault)?;
    let three=g.u32(0x30559d)?;g.set_u32(sp+76,three)?;
    let adjust=simd::subss(&mut c.mxcsr,u128::from(three),twice as u32).map_err(simd_fault)?;
    set(c,4,adjust);
    let half=g.u32(0x3055af)?;g.set_u32(sp+76,half)?;
    let scale=simd::mulss(&mut c.mxcsr,u128::from(half),approx).map_err(simd_fault)?;
    let dot=prod(c,delta,n)?;set(c,0,dot);
    let inv=simd::mulss(&mut c.mxcsr,scale,adjust as u32).map_err(simd_fault)?;set(c,1,inv);
    let normal=prod(c,splat(inv as u32),n)?;put_vector(g,c.eax,normal)?;
    let raw_depth=sum3(c,dot,2,3)?;
    let depth=simd::mulss(&mut c.mxcsr,raw_depth,inv as u32).map_err(simd_fault)?;set(c,3,depth);
    g.set_u32(sp+76,depth as u32)?;c.edx=g.u32(sp+76)?;
    c.ecx=g.u32(c.ebp+20)?;g.set_u32(c.eax+12,c.edx)?;
    let a=g.u8(c.esi)?;g.set_u8(c.ecx+8,a)?;
    let b=g.u8(c.edi)?;g.set_u8(c.ecx+9,b)?;
    c.eax=0;c.edx=c.esi+44;
    loop {
        let id=g.u16(c.edx)?;g.set_u16(c.ecx.wrapping_add(c.eax.wrapping_mul(2)),id)?;
        c.ebx=g.u32(c.esi)?;c.eax=c.eax.wrapping_add(1);c.edx=c.edx.wrapping_add(16);
        if (c.eax as i32)>=(c.ebx as i32){break;}
    }
    c.edx=0;c.eax=c.ecx.wrapping_add(c.eax.wrapping_mul(2));c.esi+=236;
    loop {
        let id=g.u16(c.esi)?;g.set_u16(c.eax,id)?;c.ecx=g.u32(c.edi)?;
        c.eax=c.eax.wrapping_add(2);c.edx=c.edx.wrapping_add(1);c.esi=c.esi.wrapping_add(16);
        arithmetic(c,c.edx,c.ecx,true);
        if (c.edx as i32)>=(c.ecx as i32){break;}
    }
    c.eax=g.u32(sp+32)?;c.edi=pop(c,g)?;c.esi=pop(c,g)?;c.ebx=pop(c,g)?;
    c.esp=c.ebp;c.ebp=pop(c,g)?;g.ret(c,20)
}
