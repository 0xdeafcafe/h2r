//! Per-user weapon animation slots, including motion tracks, idle
//! transitions and the call into the first-person palette producer.
use super::*;
use super::pose_layers::{Value,above,al,cmp,float_arg,low,push_mem};
use super::spatial::{finish_fpu,fp_error,load_fpu,scalar};
use crate::light_constants::X87;
use x87::abi::arith_op;
const TIME:u32=0x0051_0c54;
const ONE:u32=0x0045_dbc0;
fn byte_cmp(c:&mut CpuAbi,g:&Guest,p:u32,v:u8)->Result<u32>{Ok(cmp(c,u32::from(g.u8(p)?),u32::from(v),8))}
fn same(c:&mut CpuAbi,a:u32,b:u32)->Result<bool>{
    let f=simd::ucomiss(&mut c.mxcsr,a,b).map_err(super::havok::simd_fault)?;
    let ah=f|2;c.eax=(c.eax&!0xff00)|(ah<<8);
    logic(c,ah&68,128);Ok(f&68==64)
}
fn float_pop(c:&mut CpuAbi,g:&mut Guest,dst:u32,pc:u32)->Result<()>{
    let mut x=load_fpu(c);g.set_u32(dst,x.fstp_m32().map_err(fp_error)?)?;finish_fpu(c,g,x,pc,dst)
}
fn track(c:&mut CpuAbi,g:&mut Guest,v:u32,pc:u32,site:u32)->Result<TransferAbi>{
    c.eax=g.u32(c.ebp+8)?;push(c,g,c.ecx)?;c.ecx=g.u32(c.ebp-8)?;
    float_arg(c,g,Value::Memory(v),pc)?;g.call(c,0x166c39,site)
}
fn choose_motion(c:&mut CpuAbi,g:&Guest)->Result<bool>{
    Ok(byte_cmp(c,g,c.ebp-2,0)?!=0 || byte_cmp(c,g,c.ebp-1,0)?!=0)
}
pub(super) fn run(c:&mut CpuAbi,g:&mut Guest,mut pc:u32)->Result<TransferAbi>{
    loop {pc=match pc {
        0x16700c=>{
            c.ecx=g.u32(0x4ed284)?;
            c.esi=g.u32(c.ebp+8)?.wrapping_mul(148).wrapping_add(c.ecx).wrapping_add(36);
            c.eax=g.u32(c.ebx)?;logic(c,c.eax&1,128);g.set_u32(c.ebp-28,c.eax)?;
            if c.eax&1!=0 {
                scalar(c,0,g.u32(c.esi)?);push(c,g,c.ecx)?;float_arg(c,g,Value::Memory(c.ebx+8272),0x167033)?;
                return g.call(c,0xbff60,0x16703b);
            }
            c.eax=g.u32(0x4687c4)?;c.ecx=g.u32(c.eax)?;
            g.set_u32(c.ebx+8280,c.ecx)?;c.eax=g.u32(c.eax+4)?;g.set_u32(c.ebx+8284,c.eax)?;
            0x167077
        }
        0x16703b=>{
            push(c,g,c.ecx)?;g.set_u32(c.ebx+8280,low(c,0))?;float_arg(c,g,Value::Memory(c.ebx+8276),0x16704a)?;
            scalar(c,0,g.u32(c.esi+4)?);return g.call(c,0xbff60,0x167057);
        }
        0x167057=>{g.set_u32(c.ebx+8284,low(c,0))?;0x167077}
        0x167077=>{
            let a=g.u32(c.esi)?;g.set_u32(c.ebx+8272,a)?;let b=g.u32(c.esi+4)?;g.set_u32(c.ebx+8276,b)?;
            c.eax=g.u32(c.ebp-28)?|1;g.set_u32(c.ebp-8,0)?;g.set_u32(c.ebx,c.eax)?;0x167094
        }
        0x167094=>{
            c.esi=c.ebx.wrapping_add(12).wrapping_add(g.u32(c.ebp-8)?.wrapping_mul(4112));
            c.eax=c.esi+242;c.ecx=u32::from(g.u16(c.eax)?);
            g.set_u32(c.ebp-12,c.esi)?;
            if c.ecx as i16>0 {c.ecx-=1;g.set_u16(c.eax,c.ecx as u16)?;}
            let owner=arithmetic(c,g.u32(c.ebx+4)?,u32::MAX,true);g.set_u8(c.ebp-20,0)?;
            if owner!=0 && arithmetic(c,g.u32(c.esi+4)?,u32::MAX,true)!=0 {
                c.eax=g.u32(c.ebp+8)?;return g.call(c,0x155760,0x1670d0);
            }
            0x1670e8
        }
        0x1670d0=>{
            logic(c,c.eax,1<<31);
            if c.eax==0 {c.eax=c.esi+8;return g.call(c,0xb6780,0x1670dc);}
            g.set_u8(c.ebp-20,0)?;0x1670e8
        }
        0x1670dc=>{let a=al(c);g.set_u8(c.ebp-20,u8::from(a))?;0x1670e8}
        0x1670e8=>{push_mem(c,g,c.ebp-20)?;c.ecx=g.u32(c.ebp-8)?;c.eax=g.u32(c.ebp+8)?;
            return g.call(c,0x168311,0x1670f6);}
        0x1670f6=>{
            c.eax=g.u32(c.esi+4)?;
            if arithmetic(c,c.eax,u32::MAX,true)==0 {0x1675ad} else {
                c.ecx=g.u32(g.u32(points::OBJECTS.base)?+68)?;c.edi=g.u32(c.ebp-12)?;
                c.eax=g.u32(c.ecx+(c.eax&65535)*12+8)?;
                c.ecx=g.u32(c.ebp-8)?;g.set_u32(c.ebp-36,c.eax)?;
                c.eax=g.u32(0x4e9bc4)?.wrapping_add((c.ecx.wrapping_add(2*g.u32(c.ebp+8)?))<<12);
                c.ecx=g.u32(if c.ecx==0 {0x167132}else{0x167139})?;
                push(c,g,c.eax)?;c.eax=arithmetic(c,c.eax,2048,false);push(c,g,c.eax)?;
                push_mem(c,g,c.edi+768)?;c.esi=c.edi+8;push_mem(c,g,c.ebp+8)?;push(c,g,c.ecx)?;
                return g.call(c,0x1cb710,0x167156);
            }
        }
        0x167156=>{
            if al(c) {
                c.eax=u32::from(g.u8(c.edi+25)?);logic(c,c.eax&10,128);
                if c.eax&10!=0 {push_mem(c,g,c.ebp-8)?;push_mem(c,g,c.ebp+8)?;return g.call(c,0x16674e,0x16716d);}
            }
            0x16716d
        }
        0x16716d=>{c.eax=g.u32(c.ebp-32)?;return g.call(c,0x24590,0x167175);}
        0x167175=>{
            let mut x=load_fpu(c);
            g.set_u32(c.ebp-28,x.fstp_m32().map_err(fp_error)?)?;
            x.fld(g.u32(0x45dc68)?).map_err(fp_error)?;x.fld(g.u32(c.ebp-28)?).map_err(fp_error)?;
            let flags=x.fcomip(1).map_err(fp_error)?;x.fstp_st(0).map_err(fp_error)?;
            finish_fpu(c,g,x,0x167183,c.ebp-28)?;c.eflags=c.eflags&!ARITH|flags;c.eflags_defined|=ARITH;
            if flags&0x41==0 {c.ecx=g.u32(c.ebx+4)?;g.set_u8(c.ebp-2,1)?;
                return g.call(c,0xc8a10,0x167197);}
            g.set_u8(c.ebp-2,0)?;g.set_u8(c.ebp-1,0)?;0x1671a3
        }
        0x167197=>{let active=al(c);g.set_u8(c.ebp-1,u8::from(active))?;0x1671a3}
        0x1671a3=>{c.eax=g.u32(c.ebx+4)?;return g.call(c,0xd03b0,0x1671ab);}
        0x1671ab=>{
            if al(c){g.set_u8(c.ebp-2,0)?;g.set_u8(c.ebp-1,0)?;}
            if byte_cmp(c,g,c.ebp-1,0)?!=0 {
                let id=g.u32(0x1671be)?;push(c,g,id)?;c.esi=c.ebp-16;c.eax=c.edi+8;
                return g.call(c,0x1cc260,0x1671cd);
            }
            0x1671d8
        }
        0x1671cd=>{if cmp(c,u32::from(g.u16(c.ebp-14)?),65535,16)==0 {g.set_u8(c.ebp-1,0)?;}0x1671d8}
        0x1671d8=>{c.esi=c.edi+152;c.eax=c.esi;return g.call(c,0xb6760,0x1671e5);}
        0x1671e5=>{
            if !al(c) {0x1672a6} else {
                let a=byte_cmp(c,g,c.ebp-2,0)?!=0;let b=byte_cmp(c,g,c.ebp-1,0)?!=0;
                if a||b {
                    if b {scalar(c,0,g.u32(ONE)?);} else {
                        let v=g.u32(c.ebp-28)?;scalar(c,1,v);scalar(c,0,g.u32(0x45dbbc)?);
                        if !above(c,low(c,0),v)? {
                            scalar(c,0,g.u32(ONE)?);
                            if !above(c,v,low(c,0))? {c.xmm[0]=c.xmm[1];}
                        }
                    }
                    g.set_u32(c.edi+176,low(c,0))?;
                }
                push(c,g,0)?;push(c,g,0)?;c.edx=c.esi;c.ecx=c.edi+8;return g.call(c,0x1cb5b0,0x16724a);
            }
        }
        0x16724a=>{
            if choose_motion(c,g)? {
                let b=byte_cmp(c,g,c.ebp-1,0)?!=0;c.eax=g.u32(c.edi+160)?;
                let id=g.u32(0x1671be)?;let equal=arithmetic(c,c.eax,id,true)==0;
                if b==equal {0x1672a6} else {0x167272}
            } else {0x167272}
        }
        0x167272=>{c.eax=c.esi;return g.call(c,0x1c63f0,0x167279);}
        0x167279=>{
            if choose_motion(c,g)? {0x1672b2} else {
                let id=g.u32(0x16728b)?;
                if arithmetic(c,g.u32(c.edi+132)?,id,true)==0 {return track(c,g,0x4454e4,0x16729e,0x1672a6);}
                0x167307
            }
        }
        0x1672a6=>if choose_motion(c,g)? {0x1672b2}else{0x167307},
        0x1672b2=>{c.eax=c.esi;return g.call(c,0xb6760,0x1672b9);}
        0x1672b9=>{
            if al(c){0x167307}else{
                let b=byte_cmp(c,g,c.ebp-1,c.eax as u8)?!=0;
                if !b && byte_cmp(c,g,c.ebp-2,0)?==0 {0x167307} else {
                    let options=if b {u32::from(g.u8(0x1672c3)?)} else {g.u32(0x1672d2)?};
                    push(c,g,options)?;c.edi=g.u32(if b{0x1672c5}else{0x1672d7})?;
                    c.ecx=g.u32(c.ebp-24)?;push(c,g,c.esi)?;
                    c.esi=arithmetic(c,g.u32(c.ebp-12)?,8,false);
                    c.edx=g.u32(0x1672e6)?;return g.call(c,0x1cb520,0x1672ef);
                }
            }
        }
        0x1672ef=>return track(c,g,0x45dcd0,0x1672fc,0x167304),
        0x167304=>{c.edi=g.u32(c.ebp-12)?;0x167307}
        0x167307=>{c.esi=c.edi+184;c.eax=c.esi;return g.call(c,0xb6760,0x167314);}
        0x167314=>{
            if !al(c) {0x1673b4} else {
                let id=g.u32(0x167322)?;
                if arithmetic(c,g.u32(c.edi+132)?,id,true)!=0 {c.eax=c.esi;return g.call(c,0x1c63f0,0x1673b4);}
                c.eax=g.u32(c.ebp-12)?;c.ecx=c.eax+184;
                let options=g.u32(0x167336)?;push(c,g,options)?;push(c,g,c.ecx)?;
                c.ecx=g.u32(c.ebp-24)?;c.edi=g.u32(0x16733f)?;c.edx=g.u32(0x167344)?;
                c.esi=c.eax+8;return g.call(c,0x1cb520,0x167350);
            }
        }
        0x167350=>{
            let mut x=load_fpu(c);
            c.eax=g.u32(c.ebp-36)?;x.fld(g.u32(c.eax+392)?).map_err(fp_error)?;
            c.eax=g.u32(TIME)?;x.fadd_m(g.u32(ONE)?).map_err(fp_error)?;
            x.fmul_m(g.u32(c.eax+4)?).map_err(fp_error)?;
            c.eax=g.u32(c.ebp-12)?;c.esi=c.eax+184;x.arith_st(arith_op::ADD,0).map_err(fp_error)?;
            x.fld(g.u32(c.eax+212)?).map_err(fp_error)?;c.eax=c.esi;
            x.fmul_m(g.u32(0x45dc6c)?).map_err(fp_error)?;
            x.arith_reg(arith_op::ADD,1,0,true).map_err(fp_error)?;
            g.set_u32(c.ebp-36,x.fstp_m32().map_err(fp_error)?)?;
            finish_fpu(c,g,x,0x167382,c.ebp-36)?;
            return g.call(c,0x1c6ea0,0x16738a);
        }
        0x16738a=>{
            let mut x=load_fpu(c);x.fld(g.u32(c.ebp-36)?).map_err(fp_error)?;
            g.set_u32(c.ebp-40,low(c,0))?;x.fld(g.u32(c.ebp-40)?).map_err(fp_error)?;
            finish_fpu(c,g,x,0x167392,c.ebp-40)?;
            return g.call(c,0x373b6a,0x16739a);
        }
        0x16739a=>{
            let mut x=load_fpu(c);x.fmul_m(g.u32(0x45dcfc)?).map_err(fp_error)?;
            push(c,g,c.ecx)?;c.eax=c.esi;g.set_u32(c.esp,x.fstp_m32().map_err(fp_error)?)?;
            finish_fpu(c,g,x,0x1673a3,c.esp)?;
            return g.call(c,0x1c6560,0x1673ab);
        }
        0x1673ab=>0x1673b4,
        0x1673b4=>return damp(c,g),
        _=>return idle(c,g,pc),
    };}
}

fn damp(c:&mut CpuAbi,g:&mut Guest)->Result<TransferAbi>{
    c.eax=g.u32(c.ebp-12)?;
    scalar(c,5,g.u32(0x45dbfc)?);c.esp=c.esp.wrapping_sub(20);
    c.edi=c.eax+228;c.esi=c.eax+232;c.eax=g.u32(TIME)?;
    let mut x=load_fpu(c);
    for (off,v) in [(16,Value::One),(12,Value::Zero),(8,Value::Memory(0x440328)),
                    (4,Value::Memory(c.eax+4)),(0,Value::Zero)] {
        match v {
            Value::One=>X87::ok(x87::abi::ld_const(&mut x.0,x87::Const::One as u32)),
            Value::Zero=>X87::ok(x87::abi::ld_const(&mut x.0,x87::Const::Z as u32)),
            Value::Memory(p)=>x.fld(g.u32(p)?),
        }.map_err(fp_error)?;
        g.set_u32(c.esp+off,x.fstp_m32().map_err(fp_error)?)?;
        if off==8 {g.set_u32(c.ebp-28,c.edi)?;}
    }
    finish_fpu(c,g,x,0x1673f7,c.esp)?;c.edx &= !255;logic(c,0,128);
    g.call(c,0x11eed0,0x1673ff)
}
fn idle(c:&mut CpuAbi,g:&mut Guest,mut pc:u32)->Result<TransferAbi>{
    loop {pc=match pc {
        0x1673ff=>{
            scalar(c,1,0);c.eax=c.edi;scalar(c,0,g.u32(c.eax)?);
            if same(c,low(c,0),g.u32(ONE)?)?{g.set_u32(c.esi,0)?;}
            c.edi=g.u32(c.ebp-12)?;c.esi=c.edi+8;return g.call(c,0x1cd4e0,0x167424);
        }
        0x167424=>{c.eax=g.u32(c.ebp+8)?;return g.call(c,0x187420,0x16742c);}
        0x16742c=>{
            if same(c,low(c,0),low(c,1))? {c.ecx=g.u32(c.ebp+8)?;return g.call(c,0x187a40,0x167441);}
            0x167577
        }
        0x167441=>{
            let mut resting=cmp(c,c.eax,65535,16)==0;
            if resting {
                c.eax=g.u32(c.ebp-28)?;
                for p in [c.eax,c.ebx+8240,c.ebx+8244,c.ebx+8256,c.ebx+8260] {
                    scalar(c,0,g.u32(p)?);
                    if !same(c,low(c,0),low(c,1))? {resting=false;break;}
                }
            }
            if !resting {0x167577} else {
                let idle=g.u32(0x1674b9)?;
                if arithmetic(c,g.u32(c.edi+132)?,idle,true)!=0 {
                    g.set_u16(c.edi+236,0)?;logic(c,0,1<<15);0x16759d
                } else {
                    let ticks=cmp(c,u32::from(g.u16(c.edi+238)?),0,16);
                    c.eax=g.u32(g.u32(0x4e034c)?+308)?;c.esi=g.u32(0x4e7408)?;
                    g.set_u32(c.ebp-28,c.eax)?;
                    if ticks==0 {
                        push(c,g,c.ecx)?;push(c,g,c.ecx)?;
                        let mut x=load_fpu(c);
                        x.fld(g.u32(c.eax+152)?).map_err(fp_error)?;g.set_u32(c.esp+4,x.fstp_m32().map_err(fp_error)?)?;
                        c.ecx=c.esi+4;x.fld(g.u32(c.eax+148)?).map_err(fp_error)?;
                        g.set_u32(c.esp,x.fstp_m32().map_err(fp_error)?)?;
                        finish_fpu(c,g,x,0x1674f6,c.esp)?;return g.call(c,0x259d0,0x1674fe);
                    }
                    0x16750e
                }
            }
        }
        0x1674fe=>{push(c,g,c.ecx)?;float_pop(c,g,c.esp,0x1674ff)?;return g.call(c,0x1469f0,0x167507);}
        0x167507=>{g.set_u16(c.edi+238,c.eax as u16)?;0x16750e}
        0x16750e=>{
            c.eax=c.edi+236;
            let n=g.u16(c.eax)?.wrapping_add(1);g.set_u16(c.eax,n)?;
            c.eax=(c.eax&!65535)|u32::from(n);
            let limit=g.u16(c.edi+238)?;cmp(c,u32::from(n),u32::from(limit),16);
            if (n as i16)<=(limit as i16){0x16759d}else{
                g.set_u16(c.edi+238,0)?;
                let seed=g.u32(c.esi+4)?.wrapping_mul(g.u32(0x167530)?).wrapping_add(g.u32(0x167535)?);
                g.set_u32(c.esi+4,seed)?;c.eax=seed>>16;g.set_u32(c.ebp-36,c.eax)?;
                let mut x=load_fpu(c);x.fild(c.eax as i32).map_err(fp_error)?;
                x.fmul_m(g.u32(0x45dc50)?).map_err(fp_error)?;
                c.eax=g.u32(c.ebp-28)?;x.fld(g.u32(c.eax+156)?).map_err(fp_error)?;
                x.fxch(1).map_err(fp_error)?;let flags=x.fcomip(1).map_err(fp_error)?;
                x.fstp_st(0).map_err(fp_error)?;finish_fpu(c,g,x,0x167562,c.eax+156)?;
                c.eflags=c.eflags&!ARITH|flags;c.eflags_defined|=ARITH;
                if flags&1!=0 {0x16759d} else {c.eax=g.u32(0x167567)?;0x167590}
            }
        }
        0x167577=>{
            g.set_u16(c.edi+236,0)?;
            let id=g.u32(0x167585)?;
            if arithmetic(c,g.u32(c.edi+132)?,id,true)==0 {c.eax=g.u32(0x16758c)?;0x167590}else{0x16759d}
        }
        0x167590=>{
            push(c,g,1)?;push_mem(c,g,c.ebp-8)?;push_mem(c,g,c.ebp+8)?;
            return g.call(c,0x166992,0x16759d);
        }
        0x16759d=>{
            let enabled=u32::from(g.u8(c.edi)?&1);logic(c,enabled,128);
            if enabled!=0 {push_mem(c,g,c.ebp-8)?;push_mem(c,g,c.ebp+8)?;
                return g.call(c,points::FIRST_PERSON_POSE_UPDATE.base,0x1675ad);}
            0x1675ad
        }
        0x1675ad=>{
            c.eax=g.u32(c.ebp-12)?.wrapping_add(240);
            let ticks=g.u16(c.eax)?.wrapping_sub(1);g.set_u16(c.eax,ticks)?;
            cmp(c,u32::from(ticks),0,16);
            if (ticks as i16)<=0 {return g.call(c,0x13cb40,0x1675c3);}0x1675d2
        }
        0x1675c3=>{
            if !al(c) {c.ecx=g.u32(c.ebp-8)?;c.eax=g.u32(c.ebp+8)?;return g.call(c,0x16840e,0x1675d2);}
            0x1675d2
        }
        0x1675d2=>{
            let i=g.u32(c.ebp-8)?.wrapping_add(1);g.set_u32(c.ebp-8,i)?;
            if (i as i32)<2 {return run(c,g,0x167094);}
            let first=g.u8(c.ebx+12)?&1;
            c.edi=pop(c,g)?;c.esi=pop(c,g)?;
            if first==0 && g.u8(c.ebx+4124)?&1==0 {
                let flags=g.u32(c.ebx)?&!1;g.set_u32(c.ebx,flags)?;
            }
            if byte_cmp(c,g,c.ebx+8237,0)?!=0 {c.ecx=c.ebx+8236;return g.call(c,0x1d9320,0x167607);}
            0x167607
        }
        _=>return super::first_person::leave(c,g,4),
    };}
}
