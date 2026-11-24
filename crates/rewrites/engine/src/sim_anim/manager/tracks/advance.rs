//! Step the three manager channels, select the active blend, and preserve
//! the outgoing node transforms while a new transition begins.
use super::*;

const BLEND_ENABLED: u32 = 0x46fbf4;

pub(super) fn advance_one(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> Transfer {
    match pc {
        0x1cb5b0 => {
            push(c, g, c.edi)?;
            c.edi = g.u32(at(c.ecx, 0x68))?;
            c.eax = byte(c.eax, 0);
            if !eq32(c, c.edi, NONE) && valid(c, g, c.edx, NONE)? {
                if !test(c, g.u8(at(c.ecx, 0x6c))? & 1) {
                    for _ in 0..2 {
                        c.eax = frame(c, g, 12)?;
                        push(c, g, c.eax)?;
                    }
                    push(c, g, c.ecx)?;
                    c.eax = c.edx;
                    return g.call(c, 0x1c68c0, 0x1cb5df);
                }
                c.eax = byte(c.eax, 1);
            }
        }
        0x1cb5df => c.eax = byte(c.eax, 1),
        _ => return Err(bad(pc)),
    }
    c.edi = pop(c, g)?;
    g.ret(c, 8)
}

fn blend_return(c: &mut CpuAbi, g: &mut Bus<'_>) -> Transfer {
    c.esi = pop(c, g)?;
    c.eax = byte(c.eax, c.ebx as u8);
    c.ebx = pop(c, g)?;
    g.ret(c, 16)
}

pub(super) fn begin_blend(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> Transfer {
    match pc {
        0x1cb5f0 => {
            push(c, g, c.ebx)?;
            push(c, g, c.esi)?;
            c.esi = c.eax;
            c.ebx = 0;
            eq32(c, c.esi, 0);
            if c.esi as i32 <= 0 {
                return blend_return(c, g);
            }
            let duration = frame(c, g, 16)?;
            load_ss(c, 1, duration);
            if comiss(c, duration, g.u32(0x45dbd8)?) & 0x41 != 0 {
                return blend_return(c, g);
            }
            push(c, g, c.ebp)?;
            c.ebp = frame(c, g, 16)?;
            c.eax = byte(c.eax, g.u8(at(c.ebp, 0x65))?);
            c.ebp = at(c.ebp, 0x64);
            if !eq_narrow(c, c.eax, c.ebx, 8) && !test(c, g.u8(at(c.ebp, 3))? & 2) {
                c.ecx = c.ebp;
                return g.call(c, 0x1d9430, 0x1cb625);
            }
        }
        0x1cb625 => {
            if comiss(c, lane(c, 1), lane(c, 0)) & 1 != 0 {
                c.ebp = pop(c, g)?;
                return blend_return(c, g);
            }
        }
        0x1cb65d => {
            c.edi = pop(c, g)?;
            c.ebp = pop(c, g)?;
            return blend_return(c, g);
        }
        _ => return Err(bad(pc)),
    }
    let bytes = c.esi << 5;
    c.esi = frame(c, g, 28)?;
    c.eax = bytes;
    c.ecx = bytes >> 2;
    push(c, g, c.edi)?;
    c.edi = frame(c, g, 28)?;
    for off in [1, 0, 3] {
        g.set_u8(at(c.ebp, off), c.ebx as u8)?;
    }
    // The source may overlap the destination, even via a second mapping.
    // Forward each dword through the pending write log before the next load.
    let stride = if c.eflags & 0x400 != 0 {
        u32::MAX - 3
    } else {
        4
    };
    while c.ecx != 0 {
        let value = g.u32(c.esi)?;
        g.set_u32(c.edi, value)?;
        c.esi = at(c.esi, stride);
        c.edi = at(c.edi, stride);
        c.ecx -= 1;
    }
    // Byte count is always a multiple of 32, so REP MOVSB sees ECX=0.
    logic(c, 0, 1 << 31);
    c.ecx = frame(c, g, 24)?;
    push(c, g, c.ecx)?;
    c.ebx = byte(c.ebx, 1);
    c.ecx = c.ebp;
    g.call(c, 0x1d9240, 0x1cb65d)
}

pub(super) fn blend_flags(c: &mut CpuAbi, g: &mut Bus<'_>, _: u32) -> Transfer {
    c.ecx = g.u16(at(c.eax, 0x12))?.into();
    c.edx = g.u16(at(c.eax, 0x32))?.into();
    push(c, g, c.ebx)?;
    c.ebx = byte(c.ebx, g.u8(BLEND_ENABLED)?);
    push(c, g, c.esi)?;
    c.esi = NONE;
    let mut second = false;
    if test(c, c.ebx as u8) && valid(c, g, at(c.eax, 32), NONE)? {
        let lower = g.u32(0x45de20)?;
        load_ss(c, 0, lower);
        if comiss(c, lower, g.u32(at(c.eax, 128))?) != 0 {
            let blend = g.u32(at(c.eax, 128))?;
            load_ss(c, 0, blend);
            if comiss(c, blend, g.u32(0x45dd24)?) != 0 {
                c.esi = pop(c, g)?;
                c.ebx = pop(c, g)?;
                return g.ret(c, 0);
            }
            second = true;
        }
    }
    let p = at(c.eax, 0x6c);
    let primary_missing = if second {
        c.ebx = g.u32(c.eax)?;
        g.set_u8(p, g.u8(p)? | 32)?;
        c.ecx |= 0x380;
        c.edx &= !0x380;
        eq32(c, c.ebx, c.esi)
    } else {
        g.set_u8(p, g.u8(p)? & !32)?;
        push(c, g, c.edi)?;
        c.edi = g.u32(c.eax)?;
        c.ecx &= !0x380;
        c.edx |= 0x380;
        let missing = eq32(c, c.edi, c.esi);
        c.edi = pop(c, g)?;
        missing
    };
    if !primary_missing && !eq_narrow(c, g.u16(at(c.eax, 6))?.into(), c.esi, 16) {
        g.set_u16(at(c.eax, 0x12), c.ecx as u16)?;
    }
    if valid(c, g, at(c.eax, 32), c.esi)? {
        g.set_u16(at(c.eax, 0x32), c.edx as u16)?;
    }
    c.esi = pop(c, g)?;
    c.ebx = pop(c, g)?;
    g.ret(c, 0)
}

fn set_channel_defaults(c: &mut CpuAbi, g: &mut Bus<'_>) -> Result<(), GuestFault> {
    load_ss(c, 1, 0);
    let one = g.u32(ONE)?;
    load_ss(c, 0, one);
    c.eax = 0;
    logic(c, 0, 1 << 31);
    let ch = at(c.esi, 64);
    g.set_u32(ch, c.ebx)?;
    g.set_u16(at(ch, 4), c.ebx as u16)?;
    g.set_u16(at(ch, 6), c.ebx as u16)?;
    g.set_u32(at(ch, 28), 0)?;
    g.set_u8(at(ch, 16), 0)?;
    g.set_u8(at(ch, 17), 0)?;
    g.set_u16(at(ch, 18), 0)?;
    g.set_u32(at(ch, 24), one)?;
    g.set_u16(at(ch, 20), 0)?;
    g.set_u16(at(ch, 22), 0)?;
    g.set_u32(at(ch, 8), c.ebx)?;
    g.set_u8(at(ch, 12), c.ebx as u8)?;
    g.set_u8(at(ch, 13), c.ebx as u8)?;
    g.set_u16(at(ch, 14), c.ebx as u16)?;
    if valid(c, g, c.esi, c.ebx)? && test(c, g.u8(at(c.esi, 18))? & 1) {
        let p = at(c.esi, 17);
        let v = g.u8(p)? & !1;
        g.set_u8(p, v)?;
        test(c, v);
    }
    Ok(())
}

pub(super) fn manager(c: &mut CpuAbi, g: &mut Bus<'_>, mut pc: u32) -> Transfer {
    loop {
        pc = match pc {
            0x1cb710 => {
                for v in [c.ecx, c.ebx, c.ebp, c.edi] {
                    push(c, g, v)?;
                }
                c.ebp = 0;
                g.set_u8(at(c.esp, 14), 1)?;
                c.edi = c.esi;
                c.ebx = NONE;
                0x1cb720
            }
            0x1cb720 => {
                eq32(c, c.ebp, 3);
                if c.ebp as i32 >= 3 {
                    0x1cb762
                } else if valid(c, g, c.edi, c.ebx)? {
                    c.ecx = g.u32(at(c.esi, 0x68))?;
                    c.eax = byte(c.eax, 0);
                    if eq32(c, c.ecx, c.ebx) {
                        0x1cb752
                    } else if test(c, g.u8(at(c.esi, 0x6c))? & 1) {
                        0x1cb750
                    } else {
                        c.eax = frame(c, g, 24)?;
                        c.ecx = frame(c, g, 20)?;
                        for v in [c.eax, c.ecx, c.esi] {
                            push(c, g, v)?;
                        }
                        c.eax = c.edi;
                        return g.call(c, 0x1c68c0, 0x1cb750);
                    }
                } else {
                    0x1cb756
                }
            }
            0x1cb750 => {
                c.eax = byte(c.eax, 1);
                0x1cb752
            }
            0x1cb752 => {
                let p = at(c.esp, 14);
                g.set_u8(p, g.u8(p)? & c.eax as u8)?;
                0x1cb756
            }
            0x1cb756 => {
                c.eax = byte(c.eax, g.u8(at(c.esp, 14))?);
                c.ebp = at(c.ebp, 1);
                c.edi = at(c.edi, 32);
                if test(c, c.eax as u8) {
                    0x1cb720
                } else {
                    0x1cb762
                }
            }
            0x1cb762 => {
                let third = at(c.esi, 64);
                if valid(c, g, third, c.ebx)? && test(c, g.u8(at(third, 17))? & 10) {
                    c.eax = g.u32(third)?;
                    if !eq32(c, c.eax, c.ebx)
                        && !eq_narrow(c, g.u16(at(third, 6))?.into(), c.ebx, 16)
                    {
                        c.edx = g.u32(TAG_ENTRIES.base)?;
                        c.ecx = g.u32(at(third, 4))?;
                        let offset = tag_offset(c, c.eax);
                        c.eax = tag(g, c.edx, offset)?;
                        push(c, g, c.ecx)?;
                        c.edx = c.eax;
                        return g.call(c, 0x1daea0, 0x1cb7a9);
                    }
                    0x1cb7b9
                } else {
                    load_ss(c, 1, 0);
                    0x1cb82e
                }
            }
            0x1cb7a9 => {
                let copy = !(g.u8(at(c.eax, 24))? >> 1) & 1;
                c.edx = byte(c.edx, copy);
                test(c, copy);
                g.set_u8(at(c.esp, 15), copy)?;
                if copy == 0 { 0x1cb7d2 } else { 0x1cb7b9 }
            }
            0x1cb7b9 => {
                c.eax = frame(c, g, 36)?;
                c.ecx = frame(c, g, 32)?;
                push(c, g, c.eax)?;
                c.eax = frame(c, g, 32)?;
                push(c, g, c.ecx)?;
                push(c, g, g.u32(0x1cb7c8)?)?;
                push(c, g, c.esi)?;
                return g.call(c, 0x1cb5f0, 0x1cb7d2);
            }
            0x1cb7d2 => {
                set_channel_defaults(c, g)?;
                0x1cb82e
            }
            0x1cb82e => {
                c.eax = c.esi;
                return g.call(c, 0x1cb680, 0x1cb835);
            }
            0x1cb835 => {
                c.eax = byte(c.eax, g.u8(BLEND_ENABLED)?);
                if !test(c, c.eax as u8) {
                    0x1cb909
                } else {
                    c.eax = g.u32(at(c.esi, 32))?;
                    let missing = eq32(c, c.eax, c.ebx);
                    c.edi = at(c.esi, 32);
                    if missing || eq_narrow(c, g.u16(at(c.edi, 6))?.into(), c.ebx, 16) {
                        0x1cb909
                    } else {
                        c.eax = 0;
                        if !eq_narrow(c, g.u16(at(c.esi, 6))?.into(), c.ebx, 16) {
                            c.edx = g.u32(c.esi)?;
                            c.eax = g.u32(TAG_ENTRIES.base)?;
                            c.ecx = g.u32(at(c.esi, 4))?;
                            let offset = tag_offset(c, c.edx);
                            c.edx = tag(g, c.eax, offset)?;
                            push(c, g, c.ecx)?;
                            return g.call(c, 0x1daea0, 0x1cb87f);
                        }
                        0x1cb882
                    }
                }
            }
            0x1cb87f => {
                load_ss(c, 1, 0);
                0x1cb882
            }
            0x1cb882 => {
                let n = g.u16(at(c.eax, 20))?;
                c.eax = c.eax & !0xffff | u32::from(n);
                logic(c, u32::from(n), 0x8000);
                let p = at(c.esp, 36);
                g.set_u32(p, lane(c, 1))?;
                if n as i16 > 0 {
                    load_ss(c, 0, g.u32(at(c.esi, 28))?);
                    c.edx = n as i16 as u32;
                    let frames = cvtsi2ss(c, 2, c.edx);
                    let t = ss(c, Ss::Div, 0, frames);
                    let above = comiss(c, lane(c, 1), t) == 0;
                    g.set_u32(p, t)?;
                    if above {
                        g.set_u32(p, lane(c, 1))?;
                    } else {
                        load_ss(c, 1, g.u32(ONE)?);
                        if comiss(c, t, lane(c, 1)) == 0 {
                            g.set_u32(p, lane(c, 1))?;
                        }
                    }
                }
                c.eax = 0;
                if !eq_narrow(c, g.u16(at(c.edi, 6))?.into(), c.ebx, 16) {
                    c.eax = g.u32(c.edi)?;
                    c.ecx = g.u32(TAG_ENTRIES.base)?;
                    let offset = tag_offset(c, c.eax);
                    c.eax = offset;
                    c.edx = tag(g, c.ecx, offset)?;
                    c.eax = g.u32(at(c.edi, 4))?;
                    push(c, g, c.eax)?;
                    return g.call(c, 0x1daea0, 0x1cb8e4);
                }
                0x1cb8e4
            }
            0x1cb8e4 => {
                c.ecx = frame(c, g, 24)?;
                c.edx = frame(c, g, 20)?;
                c.eax = g.u16(at(c.eax, 20))? as i16 as u32;
                for v in [c.ecx, c.edx, c.esi, c.ecx] {
                    push(c, g, v)?;
                }
                cvtsi2ss(c, 0, c.eax);
                ss(c, Ss::Mul, 0, frame(c, g, 52)?);
                g.set_u32(c.esp, lane(c, 0))?;
                push(c, g, c.edi)?;
                return g.call(c, 0x1c66a0, 0x1cb909);
            }
            0x1cb909 => {
                c.eax = byte(c.eax, g.u8(at(c.esp, 14))?);
                c.edi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                c.ebx = pop(c, g)?;
                c.ecx = pop(c, g)?;
                return g.ret(c, 20);
            }
            _ => return Err(bad(pc)),
        };
    }
}
