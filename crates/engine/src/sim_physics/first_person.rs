//! First-person bone palette setup and per-axis animation filters. The
//! image owns the downstream animation graph, controllers and event calls.
use super::*;
use super::spatial::{finish_fpu,fp_error,load_fpu,scalar};
use crate::light_constants::{Sse,X87};

const USERS: u32 = 0x004e_9bc8;
const PALETTES: u32 = 0x004e_9bc4;
const IDENTITY_NODE: u32 = 0x0046_87d8;
const TIME: u32 = 0x0051_0c54;
const USER_BYTES:u32 = 8396;
const SLOT_BYTES:u32 = 4112;
pub(super) const POSE_SITES:&[u32]=&[0x16767f,0x1676ce,0x1676f5,0x1676fd,
    0x167804,0x16782d,0x167846,0x16784e,0x167888,0x16789f,0x1678c1,0x1678dc,
    0x1678e9,0x16791b,0x16793d,0x167970,0x167999,0x1679a9,0x1679d2,0x167a15,
    0x167a42,0x167a64,0x167a9f,0x167acc,0x167af9,0x167b33,0x167b5f,0x167b81,
    0x167bbc,0x167be9,0x167c0c,0x167c32,0x167c6b,0x167c7b,0x167cf8,0x167d53,
    0x167d6c,0x167d97,0x167db1,0x167dee,0x167e04];
pub(super) const POSE_REST:&[u32]=&[];
pub(super) const FILTER_SITES:&[u32]=&[0x166dbb,0x166edb,0x166f1e,0x166fcf,
    0x16700c,0x16703b,0x167057,0x1670d0,0x1670dc,0x1670f6,0x167156,0x16716d,
    0x167175,0x167197,0x1671ab,0x1671cd,0x1671e5,0x16724a,0x167279,0x1672a6,
    0x1672b9,0x1672ef,0x167304,0x167314,0x167350,0x16738a,0x16739a,0x1673ab,
    0x1673b4,0x1673ff,0x167424,0x16742c,0x167441,0x1674fe,0x167507,0x16759d,
    0x1675ad,0x1675c3,0x1675d2,0x167607];
pub(super) const FILTER_REST:&[u32]=&[];

fn frame(c:&mut CpuAbi,g:&mut Guest,locals:u32)->Result<()> {
    push(c,g,c.ebp)?;
    c.ebp=c.esp;
    c.esp=c.esp.wrapping_sub(locals);
    push(c,g,c.ebx)
}
pub(super) fn leave(c:&mut CpuAbi,g:&Guest,n:u32)->Result<TransferAbi> {
    c.ebx=pop(c,g)?;
    c.esp=c.ebp;
    c.ebp=pop(c,g)?;
    g.ret(c,n)
}
fn tag(c:&mut CpuAbi,g:&Guest,table:u32,id:u32)->Result<u32> {
    let offset=(id & 65535)<<4;
    logic(c,offset,1<<31);
    c.eflags_defined &= !(1<<11); // SHL by four
    g.u32(table.wrapping_add(offset+8))
}

pub(super) fn pose(c:&mut CpuAbi,g:&mut Guest,pc:u32)->Result<TransferAbi> {
    if pc>=0x167804 {return super::pose_layers::run(c,g,pc);}
    if pc==points::FIRST_PERSON_POSE_UPDATE.base {
        frame(c,g,120)?;
        let slot=g.u32(c.ebp+12)?.wrapping_mul(SLOT_BYTES);
        push(c,g,c.edi)?;
        c.edi=g.u32(c.ebp+8)?.wrapping_mul(USER_BYTES).wrapping_add(g.u32(USERS)?);
        c.ebx=slot.wrapping_add(c.edi).wrapping_add(12);
        c.eax=g.u32(c.ebx+4)?;
        arithmetic(c,c.eax,u32::MAX,true);
        g.set_u32(c.ebp-16,c.edi)?;
        g.set_u32(c.ebp-20,c.ebx)?;
        g.set_u32(c.ebp-36,c.eax)?;
        if c.eax==u32::MAX {c.edi=pop(c,g)?;return leave(c,g,8);}
        c.ecx=g.u32(g.u32(points::OBJECTS.base)?+68)?;
        c.edx=g.u32(points::TAG_ENTRIES.base)?;
        push(c,g,c.esi)?;
        c.esi=65535;
        c.eax=g.u32(c.ecx+(c.eax & 65535)*12+8)?;
        g.set_u32(c.ebp-32,c.eax)?;
        c.eax=tag(c,g,c.edx,g.u32(c.eax)?)?;
        g.set_u32(c.ebp-24,c.eax)?;
        c.eax=g.u32(c.edi+8)?;
        g.set_u32(c.ebp-28,c.eax)?;
        return g.call(c,0x16898b,POSE_SITES[0]);
    }
    if pc==POSE_SITES[0] {
        let weapon=g.u32(c.ebp-24)?;
        let models=g.u32(weapon+684)?;
        c.eax=g.u32(models.wrapping_add(c.eax.wrapping_shl(4)).wrapping_add(4))?;
        c.ecx=g.u32(g.u32(0x004e_034c)?+316)?;
        c.eax=tag(c,g,c.edx,c.eax & c.esi)?;
        g.set_u32(c.ebp-12,c.eax)?;
        let idx=g.u32(c.ebp-28)?.wrapping_mul(188);
        let id=g.u32(c.ecx.wrapping_add(idx).wrapping_add(4))?;
        c.eax=tag(c,g,c.edx,id & c.esi)?;
        g.set_u32(c.ebp-28,c.eax)?;
        c.eax=c.ebx+8;
        c.ecx=c.ebp-68;
        g.set_u32(c.ebp-8,c.eax)?;
        return g.call(c,0x1c62f0,POSE_SITES[1]);
    }
    if pc==POSE_SITES[1] {
        c.eax=g.u32(c.ebx+244)?;
        c.esi=g.u32(c.ebp-12)?;
        let mut unequal=arithmetic(c,c.eax,g.u32(c.esi+72)?,true)!=0;
        if !unequal {
            c.eax=g.u32(c.ebx+248)?;
            c.ecx=g.u32(c.ebp-28)?;
            unequal=arithmetic(c,c.eax,g.u32(c.ecx+72)?,true)!=0;
        }
        if unequal {
            let slot=g.u32(c.ebp+12)?;
            let user=g.u32(c.ebp+8)?;
            push(c,g,slot)?;
            return call_arg(c,g,user,0x167e86,POSE_SITES[2]);
        }
    }
    if pc!=POSE_SITES[3] {
        c.eax=g.u32(c.ebp-8)?;
        return g.call(c,0x1cbd30,POSE_SITES[3]);
    }
    palette(c,g)
}

// MOVSD transfers are ordered if the palette aliases its source. With DF
// clear and disjoint spans the normal 32-byte node fits in one guest read.
pub(super) fn copy_node<const N:usize>(g:&mut Guest,src:u32,dst:u32,backwards:bool)->Result<()> {
    if !backwards && dst.abs_diff(src)>=N as u32 && (src & 4095)+(N as u32)<=4096 && (dst & 4095)+(N as u32)<=4096 {
        let b=g.read::<N>(src)?;
        return g.write_bytes(dst,&b,1);
    }
    for i in (0..N as u32).step_by(4) {
        let offset=if backwards {0u32.wrapping_sub(i)} else {i};
        let v=g.u32(src.wrapping_add(offset))?;
        g.set_u32(dst.wrapping_add(offset),v)?;
    }
    Ok(())
}

fn palette(c:&mut CpuAbi,g:&mut Guest)->Result<TransferAbi> {
    let nodes=c.eax as i16 as i32 as u32;
    let slot=g.u32(c.ebp+12)?;
    let user=g.u32(c.ebp+8)?;
    c.eax=g.u32(PALETTES)?.wrapping_add(slot.wrapping_add(user.wrapping_mul(2)).wrapping_shl(12));
    c.ecx=nodes;
    g.set_u32(c.ebx+768,nodes)?;
    g.set_u32(c.ebp-4,c.eax)?;
    let reverse=c.eflags & 0x400!=0;
    if (nodes as i32)>0 {
        g.set_u32(c.ebp+8,c.eax)?;
        let mut i=0u32;
        loop {
            let dest=g.u32(c.ebp+8)?;
            let src=g.u32(IDENTITY_NODE)?;
            g.set_u32(c.ebp+8,dest.wrapping_add(32))?;
            push(c,g,8)?;
            c.ecx=pop(c,g)?;
            copy_node::<32>(g,src,dest,reverse)?;
            c.ecx=0;
            i=i.wrapping_add(1);
            if i as i32 >= g.u32(c.ebx+768)? as i32 {break;}
        }
        c.edi=g.u32(c.ebp-16)?;
        c.esi=g.u32(c.ebp-12)?;
    }
    for (model,palette_offset) in [(12,252),(28,508)] {
        if model==28 {c.esi=g.u32(c.ebp-28)?;}
        c.edx=0;
        arithmetic(c,g.u32(c.esi+72)?,0,true);
        g.set_u32(c.ebp+8,0)?;
        while (c.edx as i32)<(g.u32(c.esi+72)? as i32) {
            let i=c.edx;
            c.ecx=g.u32(c.esi+76)?.wrapping_add(i.wrapping_mul(96));
            let index=g.u16(c.ebx.wrapping_add(4*i).wrapping_add(palette_offset))?;
            if index!=u16::MAX {
                let dest=c.eax.wrapping_add((index as i16 as i32 as u32).wrapping_mul(32));
                scalar(c,0,g.u32(0x0045_dbc0)?);
                copy_node::<16>(g,c.ecx+24,dest,reverse)?;
                copy_node::<12>(g,c.ecx+12,dest+16,reverse)?;
                c.edi=g.u32(c.ebp-16)?;
                c.esi=g.u32(c.ebp-model)?;
                g.set_u32(dest+28,u128::from_le_bytes(c.xmm[0]) as u32)?;
            }
            let next=g.u32(c.ebp+8)?.wrapping_add(1);
            g.set_u32(c.ebp+8,next)?;
            c.edx=next as i16 as i32 as u32;
            arithmetic(c,c.edx,g.u32(c.esi+72)?,true);
        }
    }
    c.eax=g.u32(c.ebp-8)?;
    g.call(c,0xb6780,0x167804)
}

pub(super) fn filters(c:&mut CpuAbi,g:&mut Guest,pc:u32)->Result<TransferAbi> {
    if pc>=0x16700c {return super::weapon_slots::run(c,g,pc);}
    if pc==points::FIRST_PERSON_FILTER_UPDATE.base {
        frame(c,g,44)?;
        c.ebx=g.u32(USERS)?.wrapping_add(g.u32(c.ebp+8)?.wrapping_mul(USER_BYTES));
        c.eax=g.u32(c.ebx+4)?;
        arithmetic(c,c.eax,u32::MAX,true);
        if c.eax==u32::MAX {return leave(c,g,4);}
        push(c,g,c.esi)?;
        c.esi=g.u32(points::OBJECTS.base)?;
        c.ecx=g.u32(c.esi+68)?;
        push(c,g,c.edi)?;
        c.edi=65535;
        c.eax &= c.edi;
        logic(c,c.eax,1<<31);
        c.eax=g.u32(c.ecx+12*c.eax+8)?;
        g.set_u32(c.ebp-28,c.eax)?;
        c.eax=g.u32(c.ebp+8)?;
        return g.call(c,0x166561,FILTER_SITES[0]);
    }
    let index=FILTER_SITES.iter().position(|&s|s==pc).unwrap();
    if index==0 {
        for (offset,constant) in [(12,0x45e590),(8,0x45dbf8),(16,0x45e58c),(20,0x45dbfc)] {
            let v=g.u32(constant)?;
            scalar(c,0,v);
            g.set_u32(c.ebp-offset,v)?;
        }
        g.set_u32(c.ebp-24,c.eax)?;
        c.eax=c.ebx.wrapping_add(12);
        if c.eax!=0 && g.u32(c.eax+4)?!=u32::MAX {
            let handle=g.u32(c.eax+4)? & c.edi;
            let object=g.u32(g.u32(c.esi+68)?+12*handle+8)?;
            let tags=g.u32(points::TAG_ENTRIES.base)?;
            let def=g.u32(tags+16*(g.u32(object)? & c.edi)+8)?;
            scalar(c,0,0);
            for (i,off) in [12,8,16,20].into_iter().enumerate() {
                let at=def+736+4*i as u32;
                if i<3 {c.edx=at;}
                let v=g.u32(at)?;
                scalar(c,1,v);
                let cmp=simd::ucomiss(&mut c.mxcsr,v,0).map_err(super::havok::simd_fault)?;
                if cmp & 0x44!=0x40 {
                    g.set_u32(c.ebp-off,v)?;
                    if i==3 {scalar(c,0,v);}
                }
            }
        }
    }
    if index==2 {
        let dt=g.u32(g.u32(TIME)?+4)?;
        let lo=g.u32(0x45dbcc)?;
        let hi=g.u32(0x45dbc0)?;
        let mut s=Sse{mxcsr:c.mxcsr};
        let yaw=s.div(g.u32(c.ebx+8280)?,dt).map_err(fp_error)?;
        let roll=s.clamp(yaw,lo,hi).map_err(fp_error)?;
        g.set_u32(c.ebp-44,roll)?;
        let inv=s.div(lo,dt).map_err(fp_error)?;
        let pitch=s.mul(inv,g.u32(c.ebx+8284)?).map_err(fp_error)?;
        let pitch_clamped=s.clamp(pitch,lo,hi).map_err(fp_error)?;
        g.set_u32(c.ebp-40,pitch_clamped)?;
        c.mxcsr=s.mxcsr;
        scalar(c,0,lo);
        scalar(c,1,pitch);
        scalar(c,2,hi);
    }
    let clock=g.u32(TIME)?;
    let position=g.u32(c.ebp-28)?;
    let width=if index<2 {12} else {16};
    if index==0 {c.ecx=clock;c.eax=position+432;g.set_u32(c.ebp-32,c.eax)?;}
    else if index==1 {c.eax=position;} else {c.eax=clock;}
    if index==0 || index==2 {scalar(c,5,g.u32(c.ebp-if index==0 {8} else {20})?);}
    c.esp=arithmetic(c,c.esp,20,true);
    c.edi=c.ebx+[8240,8244,8256,8260][index];
    c.esi=c.edi+8;
    let mut x=load_fpu(c);
    X87::ok(x87::abi::ld_const(&mut x.0,x87::Const::One as u32)).map_err(fp_error)?;
    g.set_u32(c.esp+16,x.fstp_m32().map_err(fp_error)?)?;
    let sample=match index {0=>position+432,1=>position+436,2=>c.ebp-44,_=>c.ebp-40};
    for (source,offset) in [(0x45dbcc,12),(c.ebp-width,8),(clock+4,4),(sample,0)] {
        x.fld(g.u32(source)?).map_err(fp_error)?;
        g.set_u32(c.esp+offset,x.fstp_m32().map_err(fp_error)?)?;
    }
    if index==0 || index==2 {c.edx &= !255;logic(c,0,128);}
    finish_fpu(c,g,x,[0x166ed3,0x166f16,0x166fc7,0x167004][index],c.esp)?;
    let next=FILTER_SITES[index+1];
    g.call(c,0x11eed0,next)
}
