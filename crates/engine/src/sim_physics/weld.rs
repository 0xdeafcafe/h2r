//! Collect contact candidates, select in distance order, weld contacts
//! and issue the original agent callbacks using their guest ABI.
use super::*;
use super::havok::{put_vector,vector};
use super::spatial::{finish_fpu,fp_error,load_fpu};
use crate::light_constants::X87;

// Immediate operand at the allocator call site in the base image.
const CONTACT_POOL_CLASS_OPERAND: u32 = 0x002f_aa33;

pub(super) const SITES:&[u32]=&[0x2fa380,0x2fa3b6,0x2fa3fa,0x2fa41d,0x2fa435,0x2fa43d,
    0x2fa44f,0x2fa480,0x2fa48c,0x2fa566,0x2fa5f3,0x2fa824,0x2fa8a9,0x2fa8d6,0x2fa96a,
    0x2fa9b1,0x2faa0f,0x2faa43];
pub(super) fn supported(g:&Guest)->bool {
    matches!((g.u32(0x480154),g.u32(0x480158)),(Ok(head),Ok(end)) if head>=end)
}
fn args(c:&mut CpuAbi,g:&mut Guest,words:&[u32],target:u32,site:u32)->Result<TransferAbi> {
    for &v in words {push(c,g,v)?;}
    g.call(c,target,site)
}
fn shl7(c:&mut CpuAbi,v:u32)->u32 {
    let result=v<<7;
    logic(c,result,1<<31);
    c.eflags |= (v>>25)&1;
    c.eflags_defined &= !0x800;
    result
}

pub(super) fn contacts(c:&mut CpuAbi,g:&mut Guest,pc:u32)->Result<TransferAbi> {
    if pc==points::HAVOK_WELD_CONTACTS.base {
        push(c,g,c.ebp)?;
        c.ebp=c.esp;c.esp &= !15;
        push(c,g,u32::MAX)?;
        let exception_handler=g.u32(0x2fa2f9)?;
        push(c,g,exception_handler)?;
        c.eax=g.u32(c.fs_base)?;
        push(c,g,c.eax)?;
        g.set_u32(c.fs_base,c.esp)?;
        c.esp=c.esp.wrapping_sub(792);
        push(c,g,c.ebx)?;push(c,g,c.esi)?;
        c.esi=c.ecx;
        c.ecx=g.u32(0x480154)?;
        push(c,g,c.edi)?;
        let sp=c.esp;
        logic(c,0,128);
        g.set_u32(sp+28,c.esi)?;
        c.edi=g.u32(c.ebp+12)?;
        c.ecx=g.u32(c.edi+8)?;
        g.set_u32(sp+56,c.ecx)?;
        g.set_u32(sp+60,c.edi)?;
        c.eax=g.u32(g.u32(c.edi)?+12)?;
        c.edx=g.u32(c.edi+4)?;
        let callback=g.u32(0x2fa360)?;
        push(c,g,callback)?;
        g.set_u32(sp+48,c.eax)?;
        c.eax=g.u32(c.ebp+16)?;
        push(c,g,c.eax)?;
        c.ecx=sp+48;
        g.set_u32(sp+52,c.edx)?;
        c.edx=g.u32(c.ebp+8)?;
        push(c,g,c.ecx)?;
        push(c,g,c.edx)?;
        c.ecx=c.esi;
        return g.call(c,0x2e96a0,0x2fa380);
    }
    let sp=c.esp;
    match pc {
        0x2fa380=>{
            c.eax=sp+288;c.ebx=0;logic(c,0,1<<31);
            g.set_u32(sp+272,c.eax)?;g.set_u32(sp+276,0)?;
            let inline_capacity=g.u32(0x2fa39e)?;
            g.set_u32(sp+280,inline_capacity)?;
            c.eax=g.u32(c.esi+16)?;
            push(c,g,c.eax)?;
            c.ecx=sp+108;g.set_u32(sp+812,0)?;
            g.call(c,0x2f9c10,0x2fa3b6)
        }
        0x2fa3b6=>{
            c.eax=g.u32(c.esi+16)?;
            c.ecx=g.u32(c.edi+8)?;
            c.edx=g.u32(sp+48)?;
            g.set_u8(sp+812,1)?;
            for (off,v) in [(112,c.eax),(76,c.edi),(72,c.ecx),(36,c.edx),(40,c.ebx)] {g.set_u32(sp+off,v)?;}
            if (c.eax as i32)<=(c.ebx as i32) {return start_sort(c,g);}
            g.set_u32(sp+20,c.ebx)?;g.set_u32(sp+32,c.ebx)?;
            next_agent(c,g)
        }
        0x2fa3fa=>{
            let agent=g.u32(c.edi+8)?;
            if agent==c.eax {return advance_agent(c,g);}
            let collidable=g.u32(sp+36)?;
            c.ecx=g.u32(collidable+12)?;
            c.edx=g.u32(c.ecx)?;
            c.esi=arithmetic(c,g.u32(sp+20)?,g.u32(sp+108)?,false);
            c.eax=g.u32(c.edi)?;
            let target=g.u32(c.edx+52)?;
            args(c,g,&[c.esi,c.eax],target,0x2fa41d)
        }
        0x2fa41d=>{
            c.ebx=g.u32(g.u32(c.ebp+8)?)?;c.esi=c.eax;
            c.eax=g.u32(c.edi)?;
            g.set_u32(sp+64,c.esi)?;g.set_u32(sp+68,c.eax)?;
            c.edx=g.u32(c.esi)?;c.ecx=c.esi;
            let target=g.u32(c.edx+20)?;
            g.call(c,target,0x2fa435)
        }
        0x2fa435=>{
            push(c,g,c.eax)?;c.eax=g.u32(c.ebx)?;c.ecx=c.ebx;
            let target=g.u32(c.eax+20)?;g.call(c,target,0x2fa43d)
        }
        0x2fa43d=>{
            c.ebx=g.u32(c.ebp+16)?;c.ecx=g.u32(c.ebx)?;c.edx=sp+51;
            let target=g.u32(0x461f0c)?;
            args(c,g,&[c.eax,c.ecx,c.edx],target,0x2fa44f)
        }
        0x2fa44f=>{
            c.ecx=byte(c.ecx,g.u8(c.eax)?);
            c.esp=c.esp.wrapping_add(16);
            let sp=c.esp;
            logic(c,c.ecx & 255,128);
            if c.ecx as u8==0 {
                c.ecx=g.u32(c.ebp+20)?;c.edi=g.u32(c.edi+8)?;c.eax=g.u32(c.edi)?;
                push(c,g,c.ecx)?;c.ecx=g.u32(c.ebp+8)?;push(c,g,c.ebx)?;
                c.edx=sp+64;push(c,g,c.edx)?;push(c,g,c.ecx)?;c.ecx=c.edi;
                let target=g.u32(c.eax+16)?;return g.call(c,target,0x2fa5f3);
            }
            let input=g.u32(c.ebp+16)?;
            c.ebx=g.u32(c.edi+8)?;c.eax=g.u32(c.ebx)?;c.edx=g.u32(c.ebp+8)?;
            let target=g.u32(c.eax+32)?;
            c.ecx=c.ebx;
            args(c,g,&[sp+144,input,sp+64,c.edx,sp+24],target,0x2fa480)
        }
        0x2fa480=>{c.ecx=sp+272;g.call(c,0x2f9e80,0x2fa48c)}
        0x2fa48c=>save_candidate(c,g),
        0x2fa566=>finish_candidate(c,g),
        0x2fa5f3=>{c.esi=g.u32(sp+28)?;advance_agent(c,g)}
        0x2fa824=>{c.ebx=c.eax;advance_sort(c,g)}
        0x2fa8a9=> {
            c.eax=g.u32(c.ebp+16)?;c.ecx=g.u32(c.eax+12)?;
            let transform=vector(g,sp+128)?;
            push(c,g,c.ecx)?;c.ecx=g.u32(sp+28)?;
            c.edx=sp+272;c.eax=sp+64;
            for v in [c.ebx,0,c.edx,c.eax] {push(c,g,v)?;}
            put_vector(g,sp+80,transform)?;c.xmm[0]=transform.to_le_bytes();
            g.call(c,0x2fa0d0,0x2fa8d6)
        }
        0x2fa8d6=> {
            c.esi=arithmetic(c,c.esi,48,false);c.edi=c.edi.wrapping_sub(1);
            c.ebx=c.eax;
            if c.edi!=0 {existing_contact(c,g)} else {walk_existing(c,g,true)}
        }
        0x2fa96a=>{c.esp=c.esp.wrapping_add(28);emit_contact(c,g)}
        0x2fa9b1=>{
            c.ecx=g.u32(sp+52)?;c.eax=g.u32(sp+72)?;c.esp=c.esp.wrapping_add(32);
            c.edi=c.edi.wrapping_add(1);c.ecx=c.ecx.wrapping_add(128);
            arithmetic(c,c.edi,c.eax,true);g.set_u32(c.esp+20,c.ecx)?;
            if (c.edi as i32)<(c.eax as i32) {choose_contact(c,g)} else {destroy(c,g)}
        }
        0x2faa0f=>{
            c.eax=g.u32(sp+280)?;
            logic(c,c.eax,1<<31);g.set_u32(sp+812,u32::MAX)?;
            if (c.eax as i32)>=0 {
                c.ecx=g.u32(0x480118)?;c.edx=g.u32(c.ecx)?;
                let size=shl7(c,c.eax & 0x7fff_ffff);
                c.eax=g.u32(sp+272)?;
                let target=g.u32(c.edx+20)?;
                let class = u32::from(g.u8(CONTACT_POOL_CLASS_OPERAND)?);
                return args(c,g,&[class,size,c.eax],target,0x2faa43);
            }
            leave(c,g)
        }
        _=>leave(c,g),
    }
}

fn next_agent(c:&mut CpuAbi,g:&mut Guest)->Result<TransferAbi> {
    c.edi=arithmetic(c,g.u32(c.esi+12)?,c.ebx,false);
    g.call(c,0x301a40,0x2fa3fa)
}
fn advance_agent(c:&mut CpuAbi,g:&mut Guest)->Result<TransferAbi> {
    let sp=c.esp;
    c.eax=g.u32(sp+40)?.wrapping_add(1);
    c.ebx=g.u32(sp+32)?.wrapping_add(12);
    c.edx=g.u32(sp+20)?.wrapping_add(256);
    c.ecx=g.u32(c.esi+16)?;
    for (off,v) in [(40,c.eax),(32,c.ebx),(20,c.edx)] {g.set_u32(sp+off,v)?;}
    if (c.eax as i32)<(c.ecx as i32) {next_agent(c,g)} else {start_sort(c,g)}
}
fn save_candidate(c:&mut CpuAbi,g:&mut Guest)->Result<TransferAbi> {
    let sp=c.esp;
    let last=|g:&Guest| ->Result<u32>{Ok(g.u32(sp+272)?.wrapping_add(g.u32(sp+276)?.wrapping_shl(7)).wrapping_sub(128))};
    for (off,v) in [(112,c.ebx),(116,c.esi)] {let p=last(g)?;g.set_u32(p+off,v)?;}
    c.edi=g.u32(c.edi)?;let p=last(g)?;g.set_u32(p+120,c.edi)?;
    let p=last(g)?;let valid=g.u8(sp+24)?;g.set_u8(p+125,valid)?;
    c.eax=byte(c.eax,g.u8(sp+24)?);
    if c.eax as u8==0 {c.esi=g.u32(sp+28)?;return advance_agent(c,g);}
    c.edx=g.u32(sp+276)?.wrapping_shl(7);
    let p=g.u32(sp+272)?.wrapping_add(c.edx).wrapping_sub(128);
    for off in [0,16,48,64] {
        let v=vector(g,sp+144+off)?;
        put_vector(g,p+off,v)?;c.xmm[0]=v.to_le_bytes();
        if off==16 {
            c.ecx=g.u32(sp+176)?;g.set_u32(p+32,c.ecx)?;
            c.edx=g.u32(sp+180)?;g.set_u32(p+36,c.edx)?;
        }
    }
    c.eax=sp+208;c.ecx=arithmetic(c,g.u32(sp+72)?,32,false);
    push(c,g,c.eax)?;push(c,g,c.ecx)?;c.ecx=sp+128;
    g.call(c,0x2da730,0x2fa566)
}
fn finish_candidate(c:&mut CpuAbi,g:&mut Guest)->Result<TransferAbi> {
    let sp=c.esp;
    let p=g.u32(sp+272)?.wrapping_add(g.u32(sp+276)?.wrapping_shl(7)).wrapping_sub(128);
    let mut x=load_fpu(c);x.fld(g.u32(sp+176)?).map_err(fp_error)?;
    for (from,to) in [(192,80),(128,96)] {
        let v=vector(g,sp+from)?;put_vector(g,p+to,v)?;c.xmm[0]=v.to_le_bytes();
    }
    g.set_u32(p+92,x.fstp_m32().map_err(fp_error)?)?;
    finish_fpu(c,g,x,0x2fa599,p+92)?;
    let ty=g.u8(c.ebx+21)?;
    g.set_u8(p+124,u8::from(matches!(ty,1|2)))?;
    c.esi=g.u32(sp+28)?;
    advance_agent(c,g)
}

fn start_sort(c:&mut CpuAbi,g:&mut Guest)->Result<TransferAbi> {
    c.eax=g.u32(c.esp+276)?;c.edx=0;c.ebx=c.eax;
    g.set_u32(c.esp+20,0)?;
    if (c.ebx as i32)<=0 {return start_existing(c,g);}
    g.set_u32(c.esp+24,0)?;
    sort(c,g)
}
fn next_sorted(c:&mut CpuAbi,g:&mut Guest)->Result<bool> {
    c.eax=g.u32(c.esp+20)?.wrapping_add(1);
    c.esi=c.esi.wrapping_add(128);
    arithmetic(c,c.eax,c.ebx,true);
    g.set_u32(c.esp+20,c.eax)?;
    g.set_u32(c.esp+24,c.esi)?;
    Ok((c.eax as i32)<(c.ebx as i32))
}
fn advance_sort(c:&mut CpuAbi,g:&mut Guest)->Result<TransferAbi> {
    if next_sorted(c,g)? { sort(c,g) } else { c.eax=g.u32(c.esp+276)?;start_existing(c,g) }
}

// The comparison is strictly greater. Ties and unordered values keep
// the position already selected. Selection scans only the unsorted tail.
fn sort(c:&mut CpuAbi,g:&mut Guest)->Result<TransferAbi> {
    let sp=c.esp;
    loop {
        let head=g.u32(sp+272)?;
        let current=head.wrapping_add(g.u32(sp+24)?);
        let first=g.u32(sp+20)?;
        let mut best=current;
        let mut x=load_fpu(c);
        x.fld(g.u32(current+32)?).map_err(fp_error)?;
        for i in first..c.ebx {
            let at=head.wrapping_add(i.wrapping_mul(128));
            let distance=g.u32(at+32)?;
            X87::ok(x87::abi::cmp(&mut x.0,0,x87::abi::src_kind::F32,u64::from(distance),0,false)).map_err(fp_error)?;
            if x.0.status & 0x4100==0 {
                x.fstp_st(0).map_err(fp_error)?;
                x.fld(distance).map_err(fp_error)?;
                best=at;
            }
        }
        x.fstp_st(0).map_err(fp_error)?;
        swap(c,g,current,best,x)?;
        c.eax=g.u32(sp+272)?;
        let flag=g.u8(c.esi.wrapping_add(c.eax).wrapping_add(125))?;
        c.edx=byte(c.edx,flag);
        logic(c,u32::from(flag),128);
        c.ecx=c.esi.wrapping_add(c.eax).wrapping_add(80);
        if flag!=0 {
            c.edx=g.u32(c.ebp+16)?;c.eax=g.u32(c.edx+12)?;
            c.edx=g.u32(sp+20)?.wrapping_add(1);
            let state=c.eax;
            c.eax=sp+272;
            let contact=c.ecx;c.ecx=g.u32(sp+28)?;
            // INC edx preserves CF from the preceding TEST.
            arithmetic(c,c.edx.wrapping_sub(1),1,false);c.eflags &= !1;
            return args(c,g,&[state,c.ebx,c.edx,c.eax,contact],0x2fa0d0,0x2fa824);
        }
        if !next_sorted(c,g)? {c.eax=g.u32(sp+276)?;return start_existing(c,g);}
    }
}

fn swap(c:&mut CpuAbi,g:&mut Guest,a:u32,b:u32,mut x:X87)->Result<()> {
    let sp=c.esp;
    let fields=[0,16,48,64,80,96];
    let mut saved=[0u128;6];
    for (dst,off) in saved.iter_mut().zip(fields) {*dst=vector(g,b+off)?;}
    x.fld(g.u32(b+32)?).map_err(fp_error)?;
    x.fld(g.u32(b+36)?).map_err(fp_error)?;
    x.fxch(1).map_err(fp_error)?;
    let tail=g.u32(b+120)?;
    c.esi=g.u32(b+112)?;c.edi=g.u32(b+116)?;
    for off in [0,16] {
        let v=vector(g,a+off)?;put_vector(g,b+off,v)?;
    }
    g.set_u32(sp+264,tail)?;
    for (off,dst) in [(124,268),(125,269)] {let v=g.u8(b+off)?;g.set_u8(sp+dst,v)?;}
    for off in [32,36] {let v=g.u32(a+off)?;g.set_u32(b+off,v)?;}
    for off in [48,64,80,96] {
        let v=vector(g,a+off)?;put_vector(g,b+off,v)?;c.xmm[6]=v.to_le_bytes();
    }
    for off in [112,116,120] {c.edx=g.u32(a+off)?;g.set_u32(b+off,c.edx)?;}
    for off in [124,125] {c.edx=byte(c.edx,g.u8(a+off)?);g.set_u8(b+off,c.edx as u8)?;}
    g.set_u32(a+32,x.fstp_m32().map_err(fp_error)?)?;
    c.eax=g.u32(sp+264)?;c.edx=byte(c.edx,g.u8(sp+268)?);
    g.set_u32(a+36,x.fstp_m32().map_err(fp_error)?)?;
    finish_fpu(c,g,x,0x2fa7bd,a+36)?;
    for (i,(off,v)) in fields.into_iter().zip(saved).enumerate() {
        put_vector(g,a+off,v)?;c.xmm[i]=v.to_le_bytes();
    }
    g.set_u32(a+120,c.eax)?;
    c.eax=byte(c.eax,g.u8(sp+269)?);
    g.set_u32(a+112,c.esi)?;
    c.esi=g.u32(sp+24)?;
    g.set_u32(a+116,c.edi)?;
    g.set_u8(a+124,c.edx as u8)?;g.set_u8(a+125,c.eax as u8)
}

fn start_existing(c:&mut CpuAbi,g:&mut Guest)->Result<TransferAbi> {
    g.set_u32(c.esp+40,c.eax)?;
    if (c.eax as i32)<=0 {return start_emit(c,g);}
    g.set_u32(c.esp+24,0)?;g.set_u32(c.esp+32,c.eax)?;
    walk_existing(c,g,false)
}
fn walk_existing(c:&mut CpuAbi,g:&mut Guest,mut advance:bool)->Result<TransferAbi> {
    let sp=c.esp;
    loop {
        if advance {
            c.ecx=g.u32(sp+24)?.wrapping_add(128);
            c.eax=g.u32(sp+32)?.wrapping_sub(1);
            g.set_u32(sp+24,c.ecx)?;g.set_u32(sp+32,c.eax)?;
            if c.eax==0 {return start_emit(c,g);}
        }
        c.ecx=g.u32(sp+272)?;c.edx=g.u32(sp+24)?;
        c.esi=g.u32(c.edx.wrapping_add(c.ecx).wrapping_add(112))?;
        c.edi=g.u32(c.esi+24)?;
        if (c.edi as i32)>0 {c.esi=c.esi.wrapping_add(64);return existing_contact(c,g);}
        advance=true;
    }
}
fn existing_contact(c:&mut CpuAbi,g:&mut Guest)->Result<TransferAbi> {
    let sp=c.esp;
    let root=g.u32(c.ebp+12)?;
    let mut x=load_fpu(c);
    x.fld(g.u32(c.esi+28)?).map_err(fp_error)?;
    c.edx=g.u32(root+8)?;
    let v=vector(g,c.esi)?;
    c.xmm[0]=v.to_le_bytes();c.eax=c.esi+16;
    push(c,g,c.eax)?;
    c.edx=arithmetic(c,c.edx,32,false);
    put_vector(g,sp+64,v)?;
    g.set_u32(sp+76,x.fstp_m32().map_err(fp_error)?)?;
    finish_fpu(c,g,x,0x2fa898,sp+76)?;
    push(c,g,c.edx)?;c.ecx=sp+128;
    g.call(c,0x2da730,0x2fa8a9)
}
fn start_emit(c:&mut CpuAbi,g:&mut Guest)->Result<TransferAbi> {
    c.eax=g.u32(c.esp+40)?;c.edi=0;
    if (c.eax as i32)<=0 {return destroy(c,g);}
    g.set_u32(c.esp+20,0)?;
    choose_contact(c,g)
}
fn choose_contact(c:&mut CpuAbi,g:&mut Guest)->Result<TransferAbi> {
    let sp=c.esp;
    c.eax=g.u32(c.ebp+12)?;c.ecx=g.u32(c.eax+8)?;c.edx=g.u32(sp+20)?;
    g.set_u32(sp+76,c.eax)?;
    c.eax=g.u32(sp+272)?.wrapping_add(c.edx);
    g.set_u32(sp+72,c.ecx)?;
    c.ecx=g.u32(c.eax+120)?;c.edx=g.u32(c.eax+116)?;
    g.set_u32(sp+64,c.edx)?;g.set_u32(sp+68,c.ecx)?;
    c.esi=g.u32(c.eax+112)?;
    if (c.edi as i32)<(c.ebx as i32) {
        c.ecx=byte(c.ecx,g.u8(c.eax+125)?);
        let flag=c.ecx & 255;
        logic(c,flag,128);
        if flag!=0 {
            let contact=c.eax;
            let agent=g.u32(sp+28)?;
            let arg=g.u32(agent+8)?;
            c.edx=g.u32(c.ebp+8)?;c.eax=g.u32(c.ebp+16)?;c.ecx=sp+64;
            return args(c,g,&[c.esi+24,c.esi+48,arg,contact,c.eax,c.ecx,c.edx],0x302c70,0x2fa96a);
        }
    }
    emit_contact(c,g)
}
fn emit_contact(c:&mut CpuAbi,g:&mut Guest)->Result<TransferAbi> {
    let sp=c.esp;
    let at=g.u32(sp+272)?.wrapping_add(g.u32(sp+20)?).wrapping_add(32);
    let mut x=load_fpu(c);x.fld(g.u32(at)?).map_err(fp_error)?;
    let agent=g.u32(sp+28)?;c.eax=g.u32(agent+8)?;
    x.arith_m(x87::abi::arith_op::SUB,g.u32(0x0045_dc70)?).map_err(fp_error)?;
    c.ecx=g.u32(c.ebp+20)?;push(c,g,c.eax)?;push(c,g,c.ecx)?;
    g.set_u32(sp+36,x.fstp_m32().map_err(fp_error)?)?;
    finish_fpu(c,g,x,0x2fa98e,sp+36)?;
    c.eax=g.u32(sp+36)?;c.ecx=g.u32(c.ebp+16)?;c.edx=c.esi+24;
    push(c,g,c.edx)?;c.esi=arithmetic(c,c.esi,48,false);
    for v in [c.esi,c.eax] {push(c,g,v)?;}
    c.eax=g.u32(c.ebp+8)?;push(c,g,c.ecx)?;c.edx=sp+64;
    push(c,g,c.edx)?;push(c,g,c.eax)?;
    g.call(c,0x302eb0,0x2fa9b1)
}
fn destroy(c:&mut CpuAbi,g:&mut Guest)->Result<TransferAbi> {
    c.eax=byte(c.eax,0);logic(c,0,128);c.ecx=c.esp+108;
    g.set_u8(c.esp+812,0)?;
    g.call(c,0x2f9ca0,0x2faa0f)
}
fn leave(c:&mut CpuAbi,g:&mut Guest)->Result<TransferAbi> {
    c.ecx=g.u32(c.esp+804)?;
    c.edi=pop(c,g)?;c.esi=pop(c,g)?;
    g.set_u32(c.fs_base,c.ecx)?;
    c.ebx=pop(c,g)?;c.esp=c.ebp;c.ebp=pop(c,g)?;
    g.ret(c,16)
}
