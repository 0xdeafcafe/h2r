//! Invalidate the three channel handles, initialize a manager, and rebind a channel.
use super::*;

pub(super) fn invalidate(c: &mut CpuAbi, g: &mut Bus<'_>, _: u32) -> Transfer {
    c.eax = at(c.eax, 0x60);
    c.edx = 3;
    c.ecx = NONE;
    for _ in 0..3 {
        c.eax = arithmetic(c, c.eax, 0x20, true);
        let carry = c.eflags & 1;
        c.edx = arithmetic(c, c.edx, 1, true);
        c.eflags = c.eflags & !1 | carry;
        g.set_u32(c.eax, NONE)?;
        g.set_u16(at(c.eax, 4), u16::MAX)?;
        g.set_u16(at(c.eax, 6), u16::MAX)?;
        g.set_u32(at(c.eax, 8), NONE)?;
        g.set_u8(at(c.eax, 16), 0)?;
    }
    g.ret(c, 0)
}

pub(super) fn initialize(c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> Transfer {
    match pc {
        0x1cb0d0 => {
            push(c, g, c.ebx)?;
            c.ecx = NONE;
            c.ebx = 0;
            c.eax = byte(c.eax, 0);
            if !eq32(c, c.edx, NONE) {
                if eq32(c, c.edi, NONE) {
                    c.ebx = pop(c, g)?;
                    return g.ret(c, 4);
                }
                c.eax = g.u32(TAG_ENTRIES.base)?;
                push(c, g, c.ebp)?;
                c.ebp = tag(g, c.eax, (c.edi & 0xffff) * 16)?;
                c.ebp = g.u16(at(c.ebp, 0x0c))? as i16 as u32;
                c.edx = (c.edx & 0xffff) * 16;
                c.eax = tag(g, c.eax, c.edx)?;
                let matches = c.ebp == g.u32(at(c.eax, 0x78))?;
                c.eax = byte(c.eax, u8::from(matches));
                eq_narrow(c, c.eax, 0, 8);
                c.ebp = pop(c, g)?;
                if !matches {
                    c.eax = byte(c.eax, 0);
                    test(c, 0);
                    c.ebx = pop(c, g)?;
                    return g.ret(c, 4);
                }
            }
            let enabled = g.u8(at(c.esp, 8))? != 0;
            let flags = g.u8(at(c.esi, 0x6c))?;
            let flags = if enabled { flags | 2 } else { flags & !2 };
            g.set_u8(at(c.esi, 0x6c), flags)?;
            test(c, flags);
            for off in [0x70, 0x7c, 0x74, 0x78] {
                g.set_u32(at(c.esi, off), c.ecx)?;
            }
            g.set_u32(at(c.esi, 0x68), c.edi)?;
            load_ss(c, 0, 0);
            let one = g.u32(ONE)?;
            load_ss(c, 1, one);
            for n in 0..3 {
                let ch = at(c.esi, n * 32);
                g.set_u32(ch, NONE)?;
                g.set_u16(at(ch, 4), u16::MAX)?;
                g.set_u16(at(ch, 6), u16::MAX)?;
                if n == 1 {
                    push(c, g, g.u32(0x1cb184)?)?;
                }
                if n == 2 {
                    for operand in [0x1cb1bd, 0x1cb1c2] {
                        push(c, g, g.u32(operand)?)?;
                    }
                }
                g.set_u32(at(ch, 8), NONE)?;
                g.set_u8(at(ch, 12), 255)?;
                g.set_u8(at(ch, 13), 255)?;
                g.set_u16(at(ch, 14), u16::MAX)?;
                if n == 0 {
                    push(c, g, g.u8(0x1cb15b)?.into())?;
                }
                if n == 2 {
                    push(c, g, g.u32(0x1cb1d4)?)?;
                    c.eax = g.u32(0x1cb1d9)?;
                    c.ecx = c.esi;
                }
                g.set_u32(at(ch, 28), 0)?;
                g.set_u8(at(ch, 16), 0)?;
                g.set_u8(at(ch, 17), 0)?;
                g.set_u16(at(ch, 18), 0)?;
                g.set_u32(at(ch, 24), one)?;
                g.set_u16(at(ch, 20), 0)?;
                g.set_u16(at(ch, 22), 0)?;
            }
            g.call(c, 0x1cd070, 0x1cb200)
        }
        0x1cb200 => {
            load_ss(c, 0, 0);
            for off in [0x65, 0x64, 0x67] {
                g.set_u8(at(c.esi, off), c.ebx as u8)?;
            }
            c.eax = byte(c.eax, 1);
            g.set_u8(at(c.esi, 0x66), 1)?;
            for off in [0x61, 0x60, 0x63] {
                g.set_u8(at(c.esi, off), c.ebx as u8)?;
            }
            g.set_u16(at(c.esi, 0x6e), c.ebx as u16)?;
            g.set_u32(at(c.esi, 0x80), 0)?;
            c.ebx = pop(c, g)?;
            g.ret(c, 4)
        }
        _ => Err(bad(pc)),
    }
}

fn rebind_return(c: &mut CpuAbi, g: &mut Bus<'_>) -> Transfer {
    c.edi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 20, false);
    g.ret(c, 4)
}

pub(super) fn rebind(c: &mut CpuAbi, g: &mut Bus<'_>, mut pc: u32) -> Transfer {
    loop {
        pc = match pc {
            0x1cb230 => {
                c.esp = c.esp.wrapping_sub(20);
                push(c, g, c.ebp)?;
                c.ebp = frame(c, g, 28)?;
                push(c, g, c.edi)?;
                c.edi = g.u32(c.esi)?;
                c.eax = byte(c.eax, 0);
                let missing = eq32(c, c.edi, NONE);
                g.set_u8(at(c.esp, 32), 0)?;
                if missing || eq_narrow(c, g.u16(at(c.esi, 6))?.into(), NONE, 16) {
                    return rebind_return(c, g);
                }
                c.eax = g.u32(at(c.ebp, 0x68))?;
                c.edi = g.u32(TAG_ENTRIES.base)?;
                c.eax = (c.eax & 0xffff) * 16;
                let default = eq32(c, c.ecx, g.u32(0x1cb268)?);
                c.eax = tag(g, c.edi, c.eax)?;
                c.edi = g.u32(at(c.esi, 4))?;
                g.set_u32(at(c.esp, 8), c.edi)?;
                c.edi = g.u32(at(c.esi, 8))?;
                if default {
                    c.ecx = g.u32(at(c.ebp, 0x74))?;
                }
                if eq32(c, c.edx, g.u32(0x1cb281)?) {
                    c.edx = g.u32(at(c.ebp, 0x78))?;
                }
                push(c, g, c.ebx)?;
                let class = g.u8(at(c.esi, 14))?;
                g.set_u8(at(c.esp, 16), class)?;
                c.ebx = class as i8 as u32;
                eq32(c, c.ebx, 5);
                if c.ebx > 5 {
                    0x1cb342
                } else {
                    // Dispatch through the live graph-mode table in the image.
                    match g.u32(at(0x1cb394, c.ebx * 4))? {
                        arm @ (0x1cb2a5 | 0x1cb2c2) => {
                            for arg in [0, 0, 0, c.edx, c.ecx] {
                                push(c, g, arg)?;
                            }
                            c.ecx = g.u32(at(c.ebp, 0x70))?;
                            push(c, g, c.ecx)?;
                            c.edx = at(c.esp, if arm == 0x1cb2a5 { 44 } else { 48 });
                            push(c, g, c.edx)?;
                            push(c, g, c.eax)?;
                            c.eax = c.edi;
                            let (target, site) = if arm == 0x1cb2a5 {
                                (0x1db170, 0x1cb2be)
                            } else {
                                (0x1dbc80, 0x1cb2db)
                            };
                            return g.call(c, target, site);
                        }
                        arm @ (0x1cb2df | 0x1cb2e6) => {
                            let (size, off) = if arm == 0x1cb2df { (40, 60) } else { (20, 68) };
                            push(c, g, size)?;
                            c.eax = arithmetic(c, c.eax, off, false);
                            push(c, g, c.edi)?;
                            push(c, g, c.eax)?;
                            return g.call(c, 0x1dd560, 0x1cb2f2);
                        }
                        0x1cb342 => 0x1cb342,
                        other => return Err(bad(other)),
                    }
                }
            }
            0x1cb2be | 0x1cb2db => {
                c.eax = g.u32(c.eax)?;
                0x1cb2f9
            }
            0x1cb2f2 => {
                logic(c, c.eax, 1 << 31);
                if c.eax == 0 {
                    0x1cb342
                } else {
                    c.eax = g.u32(at(c.eax, 4))?;
                    0x1cb2f9
                }
            }
            0x1cb2f9 => {
                c.edx = c.eax >> 16;
                c.ecx = NONE;
                let missing = eq_narrow(c, c.edx, c.ecx, 16);
                g.set_u32(at(c.esp, 24), c.eax)?;
                if missing {
                    0x1cb345
                } else if eq_narrow(c, c.eax, g.u16(at(c.esp, 12))?.into(), 16)
                    && eq_narrow(c, c.edx, g.u16(at(c.esp, 14))?.into(), 16)
                {
                    c.ebx = pop(c, g)?;
                    c.edi = pop(c, g)?;
                    c.eax = byte(c.eax, 1);
                    c.ebp = pop(c, g)?;
                    c.esp = arithmetic(c, c.esp, 20, false);
                    return g.ret(c, 4);
                } else {
                    c.ecx = frame(c, g, 16)?;
                    c.ebx = g.u16(at(c.esi, 18))?.into();
                    for arg in [c.ecx, NONE, NONE, c.edi, c.eax, c.esi] {
                        push(c, g, arg)?;
                    }
                    c.eax = c.ebp;
                    return g.call(c, 0x1cb410, 0x1cb33a);
                }
            }
            0x1cb33a => {
                let result = c.eax as u8;
                test(c, result);
                g.set_u8(at(c.esp, 36), result)?;
                if result != 0 { 0x1cb389 } else { 0x1cb342 }
            }
            0x1cb342 => {
                c.ecx = NONE;
                0x1cb345
            }
            0x1cb345 => {
                load_ss(c, 0, 0);
                c.eax = 0;
                g.set_u32(c.esi, c.ecx)?;
                g.set_u16(at(c.esi, 4), c.ecx as u16)?;
                g.set_u16(at(c.esi, 6), c.ecx as u16)?;
                g.set_u32(at(c.esi, 28), 0)?;
                let one = g.u32(ONE)?;
                load_ss(c, 0, one);
                for off in [16, 17] {
                    g.set_u8(at(c.esi, off), 0)?;
                }
                for off in [18, 20, 22] {
                    g.set_u16(at(c.esi, off), 0)?;
                }
                c.eax = byte(c.eax, g.u8(at(c.esp, 36))?);
                g.set_u32(at(c.esi, 24), one)?;
                g.set_u32(at(c.esi, 8), c.ecx)?;
                for off in [12, 13] {
                    g.set_u8(at(c.esi, off), c.ecx as u8)?;
                }
                g.set_u16(at(c.esi, 14), c.ecx as u16)?;
                0x1cb389
            }
            0x1cb389 => {
                c.ebx = pop(c, g)?;
                return rebind_return(c, g);
            }
            _ => return Err(bad(pc)),
        };
    }
}
