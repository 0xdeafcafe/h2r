//! Blend controller layers into the first-person pose, then build the
//! node matrices and the primary slot's camera attachment.
use super::*;
use super::first_person::copy_node;
use super::spatial::{finish_fpu,fp_error,load_fpu,scalar};
use crate::light_constants::X87;

const MIN_AIM_FRAMES_OPERAND: u32 = 0x0016_79ad;
const MATRIX_BYTES: usize = 52;

pub(super) fn low(c:&CpuAbi,r:usize)->u32 {u128::from_le_bytes(c.xmm[r]) as u32}
fn op(c:&mut CpuAbi,r:usize,v:u32,f:fn(&mut u32,u128,u32)->std::result::Result<u128,simd::Xm>)->Result<()> {
    c.xmm[r]=f(&mut c.mxcsr,u128::from_le_bytes(c.xmm[r]),v).map_err(super::havok::simd_fault)?.to_le_bytes();Ok(())
}
pub(super) fn cmp(c:&mut CpuAbi,a:u32,b:u32,width:u32)->u32 {
    let mask=u32::MAX>>(32-width);let sign=1<<(width-1);
    let a=a&mask;let b=b&mask;let y=a.wrapping_sub(b)&mask;
    c.eflags=(c.eflags&!ARITH)|status(y,sign)|u32::from(a<b)|((a^b^y)&16)
        | (u32::from((a^b)&(a^y)&sign!=0)<<11);c.eflags_defined|=ARITH;y
}
pub(super) fn above(c:&mut CpuAbi,a:u32,b:u32)->Result<bool> {
    let f=simd::comiss(&mut c.mxcsr,a,b).map_err(super::havok::simd_fault)?;
    c.eflags=(c.eflags&!ARITH)|f;c.eflags_defined|=ARITH;Ok(f&0x41==0)
}
pub(super) fn al(c:&mut CpuAbi)->bool {let value=c.eax&255;logic(c,value,128);value!=0}
pub(super) enum Value {Memory(u32),Zero,One}
pub(super) fn float_arg(c:&mut CpuAbi,g:&mut Guest,v:Value,pc:u32)->Result<()> {
    let mut x=load_fpu(c);
    match v {
        Value::Memory(p)=>x.fld(g.u32(p)?),
        Value::Zero=>X87::ok(x87::abi::ld_const(&mut x.0,x87::Const::Z as u32)),
        Value::One=>X87::ok(x87::abi::ld_const(&mut x.0,x87::Const::One as u32)),
    }.map_err(fp_error)?;
    g.set_u32(c.esp,x.fstp_m32().map_err(fp_error)?)?;
    finish_fpu(c,g,x,pc,c.esp)
}
pub(super) fn push_mem(c:&mut CpuAbi,g:&mut Guest,p:u32)->Result<()> {let v=g.u32(p)?;push(c,g,v)}
fn cvt(c:&mut CpuAbi,r:usize,i:u32)->Result<()> {
    c.xmm[r]=simd::cvtsi2ss(&mut c.mxcsr,u128::from_le_bytes(c.xmm[r]),i)
        .map_err(super::havok::simd_fault)?.to_le_bytes();Ok(())
}
fn find_animation(c:&mut CpuAbi,g:&mut Guest,off:u32,site:u32)->Result<TransferAbi> {
    push_mem(c,g,c.edi+off)?;c.eax=c.ebp-68;push(c,g,c.eax)?;
    c.eax=g.u32(c.ebp-8)?;c.ebx=0;logic(c,0,1<<31);
    g.call(c,0x1cb490,site)
}
fn blend_args(c:&mut CpuAbi,g:&mut Guest,mode:u32)->Result<()> {
    push_mem(c,g,c.ebp-4)?;push_mem(c,g,c.edi+768)?;push(c,g,mode)?;push(c,g,c.ecx)
}

pub(super) fn run(c:&mut CpuAbi,g:&mut Guest,mut pc:u32)->Result<TransferAbi> {
    loop {pc=match pc {
        0x167804=>{
            if !al(c) {0x167d9d} else {
                c.esi=g.u32(c.ebx+768)?;c.eax=0;logic(c,0,1<<31);
                push(c,g,0)?;push(c,g,0)?;push_mem(c,g,c.ebp-4)?;
                g.set_u8(c.ebp+11,1)?;push(c,g,0)?;push(c,g,c.ecx)?;
                float_arg(c,g,Value::One,0x167821)?;push(c,g,0)?;push_mem(c,g,c.ebp-8)?;
                return g.call(c,0x1cd590,0x16782d);
            }
        }
        0x16782d=>{
            push_mem(c,g,c.ebp-4)?;push_mem(c,g,c.ebx+768)?;push(c,g,0)?;
            for off in [8,12,36] {push_mem(c,g,c.ebp-off)?;}
            return g.call(c,0xbd970,0x167846);
        }
        0x167846=>{c.ecx=g.u32(c.ebp-8)?;return g.call(c,0x1c6440,0x16784e);}
        0x16784e=>{
            logic(c,c.eax,1<<31);
            let valid=if c.eax==0 {false} else {
                let f=u32::from(g.u8(c.eax+24)?&16);logic(c,f,128);f!=0
            };
            if valid {g.set_u8(c.ebp+11,0)?;}
            let v=g.u32(if valid {0x45dc5c}else{0x45dd30})?;scalar(c,0,v);
            c.ebx=byte(c.ebx,g.u8(c.ebp+11)?);g.set_u32(c.ebp-28,v)?;
            push(c,g,c.ecx)?;c.edi=arithmetic(c,c.edi,8236,false);
            float_arg(c,g,Value::Memory(c.ebp-28),0x16787e)?;
            c.ecx=c.edi;return g.call(c,0x1d9240,0x167888);
        }
        0x167888=>{
            c.esi=g.u32(c.ebp-16)?;
            if cmp(c,u32::from(g.u8(c.esi+8237)?),0,8)!=0 {c.eax=c.edi;return g.call(c,0x1d9370,0x16789f);}
            c.edi=g.u32(c.ebp-20)?;0x167c49
        }
        0x16789f=>{
            let weight=low(c,0);
            let active=above(c,weight,g.u32(0x45dbdc)?)?;g.set_u32(c.ebp-12,weight)?;
            c.edi=g.u32(c.ebp-20)?;
            if active {c.ebx=c.edi+152;c.eax=c.ebx;return g.call(c,0xb6760,0x1678c1);}
            0x167c49
        }
        0x1678c1=>{
            if al(c) {blend_args(c,g,0)?;float_arg(c,g,Value::Memory(c.ebp-12),0x1678d4)?;
                return g.call(c,0x1c6f60,0x1678dc);}
            0x1678dc
        }
        0x1678dc=>{c.ebx=c.edi+184;c.eax=c.ebx;return g.call(c,0xb6760,0x1678e9);}
        0x1678e9=>{
            if al(c) {
                blend_args(c,g,0)?;c.eax=g.u32(c.ebp-32)?;scalar(c,0,g.u32(c.eax+392)?);
                op(c,0,g.u32(0x45dbbc)?,simd::addss)?;op(c,0,g.u32(c.ebp-12)?,simd::mulss)?;
                g.set_u32(c.esp,low(c,0))?;return g.call(c,0x1c6f60,0x16791b);
            }
            0x16791b
        }
        0x16791b=>{
            let present=cmp(c,u32::from(g.u16(c.edi+218)?),65535,16)!=0;
            g.set_u8(c.ebp+11,0)?;
            if present{return find_animation(c,g,216,0x16793d);}0x167977
        }
        0x16793d=>{
            if al(c) {
                push_mem(c,g,c.ebp-4)?;push_mem(c,g,c.edi+768)?;push(c,g,c.ebx)?;
                c.esp=arithmetic(c,c.esp,12,true);
                let mut x=load_fpu(c);
                for (off,src) in [(8,c.ebp-12),(4,c.esi+8276),(0,c.esi+8256)] {
                    x.fld(g.u32(src)?).map_err(fp_error)?;g.set_u32(c.esp+off,x.fstp_m32().map_err(fp_error)?)?;
                }
                finish_fpu(c,g,x,0x167968,c.esp)?;c.esi=c.ebp-68;
                return g.call(c,0x1c7100,0x167970);
            }
            0x167977
        }
        0x167970=>{c.esi=g.u32(c.ebp-16)?;g.set_u8(c.ebp+11,1)?;0x167977}
        0x167977=>{
            if cmp(c,u32::from(g.u16(c.edi+222)?),65535,16)!=0{return find_animation(c,g,220,0x167999);}
            0x167c49
        }
        0x167999=>{if al(c){c.ecx=c.ebp-68;return g.call(c,0x1c6440,0x1679a9);}0x167c49}
        0x1679a9=>{
            let frames=g.u16(c.eax+20)? as i16;
            let minimum = i16::from(g.u8(MIN_AIM_FRAMES_OPERAND)? as i8);
            cmp(c,frames as u32,minimum as u32,16);
            if frames>=minimum {0x1679b4} else {
                g.set_u16(c.edi+220,65535)?;g.set_u16(c.edi+222,65535)?;logic(c,65535,1<<15);0x167c49
            }
        }
        0x1679b4|0x167a42|0x167ad7|0x167b5f=> {
            let (off,pos,neg,next)=match pc {
                0x1679b4=>(8240,0x1679d2,0x167a15,0x167a42),
                0x167a42=>(8244,0x167a64,0x167a9f,0x167acc),
                0x167ad7=>(8256,0x167af9,0x167b33,0x167b5f),
                _=>(8260,0x167b81,0x167bbc,0x167be9),
            };
            let v=g.u32(c.esi+off)?;scalar(c,1,v);scalar(c,0,0);
            let positive=above(c,v,0)?;
            if positive || above(c,0,v)? {
                let (constant,store)=match (off,positive) {
                    (8240,true)=>(Value::Zero,0x1679ca),(8240,false)=>(Value::One,0x167a0d),
                    (8244,true)=>(Value::Memory(0x45dbd4),0x167a5c),(8244,false)=>(Value::Memory(0x45dc0c),0x167a97),
                    (8256,true)=>(Value::Memory(0x45dc10),0x167af1),(8256,false)=>(Value::Memory(0x45dbf0),0x167b2b),
                    (_,true)=>(Value::Memory(0x45dda8),0x167b79),(_,false)=>(Value::Memory(0x45dbfc),0x167bb4),
                };
                push(c,g,c.ecx)?;c.eax=c.ebp-68;float_arg(c,g,constant,store)?;
                return g.call(c,0x1c6560,if positive{pos}else{neg});
            }
            next
        }
        0x1679d2|0x167a15|0x167a64|0x167a9f|0x167af9|0x167b33|0x167b81|0x167bbc=>{
            let (off,negative,site)=match pc {
                0x1679d2|0x167a15=>(8240,pc==0x167a15,0x167a42),
                0x167a64|0x167a9f=>(8244,pc==0x167a9f,0x167acc),
                0x167af9|0x167b33=>(8256,pc==0x167b33,0x167b5f),
                _=>(8260,pc==0x167bbc,0x167be9),
            };
            let mode=if off==8256 || pc==0x1679d2 {c.ebx} else {0};
            blend_args(c,g,mode)?;
            let weight=g.u32(c.ebp-12)?;let value=g.u32(c.esi+off)?;
            scalar(c,0,if off==8240 {weight}else{value});
            op(c,0,if off==8240 {value}else{weight},simd::mulss)?;
            if negative {scalar(c,1,0);op(c,1,low(c,0),simd::subss)?;}
            g.set_u32(c.esp,low(c,usize::from(negative)))?;
            c.ebx=c.ebp-68;return g.call(c,0x1c6f60,site);
        }
        0x167acc=>{c.ebx=0;if cmp(c,u32::from(g.u8(c.ebp+11)?),0,8)!=0 {0x167be9}else{0x167ad7}}
        0x167be9=>{
            let v=g.u32(c.edi+228)?;scalar(c,0,v);
            if above(c,v,g.u32(0x45dbd8)?)? {push(c,g,c.ecx)?;c.eax=c.ebp-68;
                float_arg(c,g,Value::Memory(0x45dc18),0x167c04)?;return g.call(c,0x1c6560,0x167c0c);}
            0x167c49
        }
        0x167c0c=>{
            blend_args(c,g,0)?;scalar(c,0,g.u32(c.edi+228)?);op(c,0,g.u32(c.ebp-12)?,simd::mulss)?;
            c.ebx=c.ebp-68;g.set_u32(c.esp,low(c,0))?;return g.call(c,0x1c6f60,0x167c32);
        }
        0x167c32=>0x167c49,
        0x167c49=>{
            if cmp(c,u32::from(g.u16(c.edi+226)?),65535,16)!=0 {return find_animation(c,g,224,0x167c6b);}
            0x167d6c
        }
        0x167c6b=>{if al(c){c.ecx=c.ebp-68;return g.call(c,0x1c6440,0x167c7b);}0x167d6c}
        0x167c7b=>return frame_index(c,g),
        0x167cf8=>{
            c.ebx=g.u16(c.ebx+10)? as i16 as i32 as u32;c.ecx=g.u16(c.ebp+8)? as i16 as i32 as u32;
            if c.eax as i16>c.ebx as i16 {c.eax=c.ebx;}
            c.eax=(c.eax as i16 as i32 as u32).wrapping_sub(c.ecx);cvt(c,1,c.eax)?;
            op(c,1,low(c,0),simd::mulss)?;
            let v=low(c,1);c.eax=simd::cvttss2si(&mut c.mxcsr,v).map_err(super::havok::simd_fault)?;
            let n=arithmetic(c,g.u32(c.ebp+8)?,c.eax,false);g.set_u32(c.ebp+8,n)?;
            c.eax=g.u16(c.ebp+8)? as i16 as i32 as u32;return set_frame(c,g);
        }
        0x167d53=>{
            blend_args(c,g,0)?;c.ebx=c.ebp-68;float_arg(c,g,Value::One,0x167d64)?;
            return g.call(c,0x1c6f60,0x167d6c);
        }
        0x167d6c=>{
            if g.u8(c.esi)?&2!=0 {
                c.eax=g.u32(c.ebp-8)?;
                if g.u8(c.eax+101)?!=0 {
                    c.eax=g.u32(c.ebp-4)?;c.edx=g.u32(c.ebp-20)?;
                    c.edx=(c.edx&!65535)|u32::from(g.u16(c.edx+768)?);
                    push(c,g,c.eax)?;push_mem(c,g,c.ebp-8)?;
                    c.eax=c.eax.wrapping_add(2048);c.ecx=0;logic(c,0,1<<31);
                    return g.call(c,0x1cc2b0,0x167d97);
                }
            }
            0x167d97
        }
        0x167d97=>{c.ebx=g.u32(c.ebp-20)?;c.edi=g.u32(c.ebp-16)?;0x167d9d}
        0x167d9d=>{
            c.ecx=g.u32(c.ebp-24)?;let flags=g.u32(c.edi)?|2;g.set_u32(c.edi,flags)?;
            c.ecx=arithmetic(c,c.ecx,776,false);c.eax=c.ebp-120;return g.call(c,0x1416c0,0x167db1);
        }
        0x167db1=>{
            c.ecx=(c.ecx&!65535)|u32::from(g.u16(c.ebx+764)?);
            let mut both=cmp(c,c.ecx,65535,16)!=0;
            c.eax=g.u32(c.ebx+768)?;g.set_u32(c.ebx+772,c.eax)?;
            if both {c.eax=u32::from(g.u16(c.ebx+766)?);both=cmp(c,c.eax,65535,16)!=0;}
            if both {
                push_mem(c,g,c.ebp-4)?;c.edx=c.ebx+776;push(c,g,c.edx)?;push_mem(c,g,c.ebp-8)?;
                c.edx=c.ebp-120;return g.call(c,0x1cbd80,0x167dee);
            }
            push_mem(c,g,c.ebp-8)?;c.ecx=g.u32(c.ebp-4)?;c.edx=c.ebx+776;c.eax=c.ebp-120;
            return g.call(c,0x1cbd50,0x167e04);
        }
        _=>return finish(c,g),
    };}
}

fn set_frame(c:&mut CpuAbi,g:&mut Guest)->Result<TransferAbi>{
    cvt(c,0,c.eax)?;push(c,g,c.ecx)?;c.eax=c.ebp-68;g.set_u32(c.esp,low(c,0))?;
    g.call(c,0x1c6560,0x167d53)
}

fn frame_index(c:&mut CpuAbi,g:&mut Guest)->Result<TransferAbi> {
    c.edx=g.u32(c.ebp-24)?;
    let mut reload=g.u16(c.edx+658)?==2;
    if reload {
        c.ecx=g.u32(g.u32(c.ebp-8)?+124)?;
        // Private animation identifiers live in the two comparison operands.
        reload=c.ecx==g.u32(0x167c94)? || c.ecx==g.u32(0x167c9c)?;
    }
    c.ecx=g.u32(c.ebp-32)?;
    let counter=g.u16(c.ecx+556)?;
    if reload {
        g.set_u16(c.ebp+8,counter)?;
        let elapsed=g.u16(c.ecx+552)?.wrapping_sub(g.u16(c.ecx+550)?);
        let delay=u32::from(g.u8(0x167cc1)?);
        cmp(c,u32::from(elapsed),delay,16);
        if i32::from(elapsed as i16)>=delay as i32 {
            scalar(c,1,g.u32(0x45dbc0)?);c.ebx=g.u32(c.edx+708)?;
            c.eax=u32::from(elapsed).wrapping_sub(delay);cvt(c,0,c.eax)?;
            op(c,0,g.u32(0x45dc5c)?,simd::mulss)?;
            if above(c,low(c,0),low(c,1))? {c.xmm[0]=c.xmm[1];}
            c.edx=g.u32(c.ebp-36)?;push(c,g,0)?;c.eax=0;logic(c,0,1<<31);
            return g.call(c,0x100b40,0x167cf8);
        }
        c.eax=g.u16(c.ebp+8)? as i16 as i32 as u32;
    } else {
        c.ecx=(c.ecx & !65535)|u32::from(counter);
        if (counter as i16)<0 {c.eax=0;logic(c,0,1<<31);} else {
            c.eax=(g.u16(c.eax+20)? as i16 as i32 as u32).wrapping_sub(1);
            c.ecx=u32::from(counter);arithmetic(c,c.ecx,c.eax,true);
            if (c.ecx as i32)<=(c.eax as i32){c.eax=c.ecx;}
        }
    }
    set_frame(c,g)
}

fn finish(c:&mut CpuAbi,g:&mut Guest)->Result<TransferAbi> {
    if arithmetic(c,g.u32(c.ebp+12)?,0,true)==0 {
        c.eax=g.u32(c.edi+8340)?;
        if c.eax==u32::MAX {
            let flags=g.u32(c.edi)? & !4;logic(c,flags,1<<31);g.set_u32(c.edi,flags)?;
        } else {
            c.edx=g.u32(c.ebp-24)?;
            c.esi=c.eax.wrapping_mul(MATRIX_BYTES as u32).wrapping_add(c.ebx).wrapping_add(776);
            push(c,g,(MATRIX_BYTES/4) as u32)?;c.eax=g.u32(c.ebp-16)?;c.edi+=8344;c.ecx=pop(c,g)?;
            copy_node::<MATRIX_BYTES>(g,c.esi,c.edi,c.eflags&0x400!=0)?;
            for (i,off) in [8384,8388,8392].into_iter().enumerate() {
                c.ecx=c.eax+off;scalar(c,0,g.u32(c.ecx)?);
                if i==0 {c.edx+=776;} else {c.edx=g.u32(c.ebp-24)?;}
                let rhs=if i==0 {c.edx} else {c.edx+776+4*i as u32};
                op(c,0,g.u32(rhs)?,simd::subss)?;g.set_u32(c.ecx,low(c,0))?;
            }
            let flags=g.u32(c.eax)?|4;g.set_u32(c.eax,flags)?;logic(c,flags,1<<31);
        }
    }
    c.esi=pop(c,g)?;c.edi=pop(c,g)?;c.ebx=pop(c,g)?;c.esp=c.ebp;c.ebp=pop(c,g)?;
    g.ret(c,8)
}
