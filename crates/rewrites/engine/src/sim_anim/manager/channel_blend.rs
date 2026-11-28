//! Transition matching and installation. All three channels and the transition
//! search records reside in guest memory across graph and event callbacks.
use super::*;
use crate::sim_native::sub_flags;
use crate::sim_physics::{ARITH, arithmetic, byte, logic};

pub(super) const BODIES: &[Body] = &[
    body!(
        0x1ccda0,
        0x1cd070,
        &[0x1cce8d, 0x1ccec6, 0x1ccf3f, 0x1ccf89],
        find_transition
    ),
    body!(
        0x1cd070,
        0x1cd4dd,
        &[
            0x1cd0d7, 0x1cd0f4, 0x1cd119, 0x1cd167, 0x1cd192, 0x1cd206, 0x1cd2ab, 0x1cd2b8,
            0x1cd2d2, 0x1cd2e6, 0x1cd310, 0x1cd329, 0x1cd336, 0x1cd347, 0x1cd354, 0x1cd35c,
            0x1cd385, 0x1cd3a8, 0x1cd3e7, 0x1cd469
        ],
        install_transition
    ),
];

type Checked<T> = std::result::Result<T, GuestFault>;
const ONE: u32 = 0x45dbc0;
const SMALL: u32 = 0x45dbdc;
const EPSILON: u32 = 0x45dbd8;

fn word(reg: u32, low: u16) -> u32 {
    (reg & !0xffff) | u32::from(low)
}

fn slot_offset(c: &mut CpuAbi, handle: u32) -> u32 {
    let offset = (handle & 0xffff) << 4;
    logic(c, offset, 1 << 31);
    c.eflags_defined &= !0x800; // shl ecx,4 leaves OF undefined.
    offset
}

fn set_ss(c: &mut CpuAbi, r: usize, v: u32) {
    c.xmm[r] = u128::from(v).to_le_bytes();
}

fn ss(c: &CpuAbi, r: usize) -> u32 {
    u128::from_le_bytes(c.xmm[r]) as u32
}

type SsOp = fn(&mut u32, u128, u32) -> std::result::Result<u128, simd::Xm>;

fn binary(c: &mut CpuAbi, r: usize, rhs: u32, op: SsOp) -> Checked<()> {
    c.xmm[r] = op(&mut c.mxcsr, u128::from_le_bytes(c.xmm[r]), rhs)
        .map_err(|_| bad(c.eip))?
        .to_le_bytes();
    Ok(())
}

fn above(c: &mut CpuAbi, a: u32, b: u32) -> Checked<bool> {
    let flags = simd::comiss(&mut c.mxcsr, a, b).map_err(|_| bad(c.eip))?;
    c.eflags = (c.eflags & !ARITH) | flags;
    c.eflags_defined |= ARITH;
    Ok(flags & 0x41 == 0)
}

fn integer_to_ss(c: &mut CpuAbi, r: usize, value: u32) -> Checked<()> {
    c.xmm[r] = simd::cvtsi2ss(&mut c.mxcsr, u128::from_le_bytes(c.xmm[r]), value)
        .map_err(|_| bad(c.eip))?
        .to_le_bytes();
    Ok(())
}

/// The initial graph lookup saves EDI and uses the possibly remapped action.
/// The repeated lookup supplies its pass number and reads the original action.
fn lookup_transition(c: &mut CpuAbi, g: &mut Bus<'_>, repeated: bool) -> sim_native::Result {
    let sp = c.esp;
    if repeated {
        c.ecx = g.u32(at(sp, 0x3c))?;
        c.edx = g.u32(at(sp, 0x38))?;
        c.eax = g.u32(at(c.ebx, 0x7c))?;
        for arg in [c.ebp, c.esi, c.esi, c.ecx] {
            push(c, g, arg)?;
        }
        c.ecx = g.u32(at(c.ebx, 0x78))?;
        push(c, g, c.edx)?;
        c.edx = g.u32(at(c.ebx, 0x70))?;
        for arg in [c.esi, c.esi, c.eax, c.ecx] {
            push(c, g, arg)?;
        }
        c.ecx = g.u32(at(c.ebx, 0x68))?;
    } else {
        c.edx = g.u32(at(sp, 0x34))?;
        c.eax = g.u32(at(sp, 0x30))?;
        for arg in [c.edi, 0, c.esi, c.esi, c.edx] {
            push(c, g, arg)?;
        }
        c.edx = g.u32(at(c.ebx, 0x70))?;
        for arg in [c.eax, c.esi, c.esi, c.ecx] {
            push(c, g, arg)?;
        }
        c.ecx = g.u32(at(c.ebx, 0x78))?;
        push(c, g, c.ecx)?;
        c.ecx = g.u32(at(c.ebx, 0x68))?;
    }
    push(c, g, c.edx)?;
    c.edx = g.u32(guest_layout::points::TAG_ENTRIES.base)?;
    c.eax = at(c.esp, if repeated { 0x48 } else { 0x4c });
    push(c, g, c.eax)?;
    c.ecx = slot_offset(c, c.ecx);
    c.eax = g.u32(at(at(c.ecx, c.edx), 8))?;
    push(c, g, c.eax)?;
    c.eax = g.u32(at(c.ebx, 0x74))?;
    g.call(c, 0x1dc8c0, if repeated { 0x1ccf3f } else { 0x1cce8d })
}

/// Search the four transition event kinds at the channel’s current frame.
/// Both a generic match and an event-timed match live in caller stack slots.
fn find_transition(c: &mut CpuAbi, g: &mut Bus<'_>, mut pc: u32) -> sim_native::Result {
    loop {
        let sp = c.esp;
        pc = match pc {
            0x1ccda0 => {
                c.esp = sp.wrapping_sub(0x24);
                c.eax = g.u32(at(c.esp, 0x34))?;
                g.set_u8(c.eax, 0)?;
                c.eax = byte(c.eax, g.u8(0x46fbf5)?);
                push(c, g, c.esi)?;
                c.esi = u32::MAX;
                g.set_u16(at(c.esp, 0x14), c.esi as u16)?;
                g.set_u16(at(c.esp, 0x16), c.esi as u16)?;
                if c.eax as u8 == 0 {
                    c.edx = g.u32(at(c.esp, 0x14))?;
                    c.eax = g.u32(at(c.esp, 0x2c))?;
                    g.set_u32(c.eax, c.edx)?;
                    c.esi = pop(c, g)?;
                    c.esp = arithmetic(c, c.esp, 0x24, false);
                    return g.ret(c, 16);
                }
                set_ss(c, 0, g.u32(at(c.ebx, 0x80))?);
                binary(c, 0, g.u32(SMALL)?, simd::addss)?;
                set_ss(c, 1, 0);
                let below = above(c, ss(c, 1), ss(c, 0))?;
                c.ecx = g.u32(at(c.ebx, 0x7c))?;
                if below {
                    c.xmm[0] = c.xmm[1];
                } else {
                    set_ss(c, 2, g.u32(0x45dc78)?);
                    if above(c, ss(c, 0), ss(c, 2))? {
                        c.xmm[0] = c.xmm[2];
                    }
                }
                binary(c, 0, g.u32(0x45dc74)?, simd::mulss)?;
                if above(c, ss(c, 1), ss(c, 0))? {
                    c.xmm[0] = c.xmm[1];
                } else {
                    set_ss(c, 1, g.u32(ONE)?);
                    if above(c, ss(c, 0), ss(c, 1))? {
                        c.xmm[0] = c.xmm[1];
                    }
                }
                c.eax = byte(c.eax, g.u8(0x46fbf4)?);
                if c.eax as u8 != 0
                    && g.u32(at(c.ebx, 0x20))? != c.esi
                    && g.u16(at(c.ebx, 0x26))? != c.esi as u16
                    && above(c, ss(c, 0), g.u32(0x45dd24)?)?
                {
                    for index in 0..4 {
                        if c.ecx == g.u32(at(0x46fbf8, index * 8))? {
                            let alternate = g.u32(at(0x46fbfc, index * 8))?;
                            if alternate != c.esi {
                                c.ecx = alternate;
                            }
                            break;
                        }
                    }
                }
                return lookup_transition(c, g, false);
            }
            0x1cce8d => {
                let missing = g.u16(at(c.ebx, 6))? == c.esi as u16;
                c.edi = word(c.edi, c.esi as u16);
                g.set_u16(at(sp, 0x10), c.esi as u16)?;
                g.set_u16(at(sp, 0x12), c.edi as u16)?;
                g.set_u32(at(sp, 0x14), 0)?;
                if !missing {
                    c.ecx = g.u32(c.ebx)?;
                    c.edx = g.u32(guest_layout::points::TAG_ENTRIES.base)?;
                    c.eax = g.u32(at(c.ebx, 4))?;
                    c.ecx = slot_offset(c, c.ecx);
                    c.edx = g.u32(at(at(c.ecx, c.edx), 8))?;
                    push(c, g, c.eax)?;
                    return g.call(c, 0x1daea0, 0x1ccec6);
                }
                0x1cceca
            }
            0x1ccec6 => {
                g.set_u32(at(sp, 0x14), c.eax)?;
                0x1cceca
            }
            0x1cceca => {
                c.eax = g.u32(at(sp, 0x14))?;
                g.set_u8(at(sp, 0xb), 0)?;
                if c.eax == 0 {
                    0x1cd003
                } else {
                    set_ss(c, 0, g.u32(at(c.ebx, 0x1c))?);
                    push(c, g, c.ebp)?;
                    g.set_u32(at(c.esp, 0x24), ss(c, 0))?;
                    set_ss(c, 0, g.u32(at(c.esp, 0x24))?);
                    c.eax = simd::cvttss2si(&mut c.mxcsr, u128::from_le_bytes(c.xmm[0]) as u32)
                        .map_err(|_| bad(pc))?;
                    c.ebp = 1;
                    g.set_u32(at(c.esp, 0x2c), c.eax)?;
                    g.set_u32(at(c.esp, 0x24), c.ebp)?;
                    return lookup_transition(c, g, true);
                }
            }
            0x1ccf3f => {
                if g.u16(at(sp, 0x22))? == c.esi as u16 {
                    0x1ccfef
                } else {
                    c.edi = 5;
                    g.set_u32(at(sp, 0x10), 5)?;
                    // Event kinds five to eight correspond to the four lookup passes.
                    if (1..=4).contains(&c.ebp) {
                        c.edi = c.ebp + 4;
                        g.set_u32(at(sp, 0x10), c.edi)?;
                    }
                    sub_flags(
                        c,
                        c.ebp,
                        if (1..=4).contains(&c.ebp) { c.ebp } else { 4 },
                        32,
                    );
                    c.eax = g.u32(at(sp, 0x18))?;
                    return g.call(c, 0x1dade0, 0x1ccf89);
                }
            }
            0x1ccf89 => {
                c.ebp = c.eax as i16 as i32 as u32;
                if c.ebp != c.esi {
                    c.edi = g.u32(at(sp, 0x20))?;
                    match_event(c, g)?;
                }
                c.ebp = g.u32(at(sp, 0x24))?;
                0x1ccfef
            }
            0x1ccfef => {
                c.ebp = c.ebp.wrapping_add(1);
                sub_flags(c, c.ebp, 4, 32);
                g.set_u32(at(sp, 0x24), c.ebp)?;
                if (c.ebp as i32) <= 4 {
                    return lookup_transition(c, g, true);
                }
                c.edi = word(c.edi, g.u16(at(sp, 0x16))?);
                c.ebp = pop(c, g)?;
                0x1cd003
            }
            0x1cd003 => {
                let candidate = g.u16(at(sp, 0x26))?;
                if candidate != c.esi as u16 && c.edi as u16 == c.esi as u16 {
                    c.eax = g.u32(at(sp, 0x24))?;
                    c.ecx = g.u32(at(sp, 0x30))?;
                    c.edi = pop(c, g)?;
                    g.set_u32(c.ecx, c.eax)?;
                    c.eax = c.ecx;
                } else {
                    if c.edi as u16 == c.esi as u16 {
                        c.eax = g.u32(at(sp, 0x3c))?;
                        c.edx = byte(c.edx, g.u8(at(sp, 0xb))?);
                        c.ecx = g.u32(at(sp, 0x18))?;
                        g.set_u8(c.eax, c.edx as u8)?;
                        c.eax = g.u32(at(sp, 0x30))?;
                    } else {
                        c.ecx = g.u32(at(sp, 0x10))?;
                        c.eax = g.u32(at(sp, 0x30))?;
                    }
                    c.edi = pop(c, g)?;
                    g.set_u32(c.eax, c.ecx)?;
                }
                c.esi = pop(c, g)?;
                c.esp = arithmetic(c, c.esp, 0x24, false);
                return g.ret(c, 16);
            }
            _ => return Err(bad(pc)),
        };
    }
}

/// Find the next event of this kind after EBP, until a frame falls in the
/// transition window. A corrupt table can cycle through the caller's stack.
fn match_event(c: &mut CpuAbi, g: &mut Bus<'_>) -> Checked<()> {
    let sp = c.esp;
    let mut probes = 0;
    while g.u16(at(sp, 0x16))? == c.esi as u16 {
        probes += 1;
        if probes > 65536 {
            return Err(bad(0x1ccf94));
        }
        c.eax = g.u32(at(sp, 0x2c))?.wrapping_sub(c.ebp);
        g.set_u8(at(sp, 0xf), 1)?;
        if (-2..=0).contains(&(c.eax as i32)) {
            g.set_u32(at(sp, 0x14), c.edi)?;
        } else {
            c.eax = g.u32(at(sp, 0x18))?;
            c.edx = g.u32(at(c.eax, 0x4c))?;
            c.ecx = 0;
            if c.edx as i32 > 0 {
                c.eax = g.u32(at(c.eax, 0x50))?;
                loop {
                    probes += 1;
                    if probes > 65536 {
                        return Err(bad(0x1ccfc6));
                    }
                    c.esi = g.u16(c.eax)? as i16 as i32 as u32;
                    if c.esi == g.u32(at(sp, 0x10))? {
                        c.esi = g.u16(at(c.eax, 2))? as i16 as i32 as u32;
                        if (c.esi as i32) > (c.ebp as i32) {
                            c.eax = word(c.eax, g.u16(at(c.eax, 2))?);
                            c.esi = u32::MAX;
                            break;
                        }
                    }
                    c.ecx += 1;
                    c.eax = at(c.eax, 4);
                    if c.ecx == c.edx {
                        c.esi = u32::MAX;
                        c.eax = c.esi;
                        break;
                    }
                }
            } else {
                c.eax = c.esi;
            }
            c.ebp = c.eax as i16 as i32 as u32;
        }
        if c.ebp == c.esi {
            break;
        }
    }
    Ok(())
}

fn mask_byte(c: &mut CpuAbi, g: &mut Bus<'_>, addr: u32, and: u8, or: u8) -> Checked<()> {
    let result = (g.u8(addr)? & and) | or;
    g.set_u8(addr, result)?;
    logic(c, result.into(), 0x80);
    Ok(())
}

fn playback_fraction(c: &mut CpuAbi, g: &mut Bus<'_>) -> Checked<()> {
    let sp = c.esp;
    c.eax = g.u16(at(c.eax, 0x14))? as i16 as i32 as u32;
    set_ss(c, 0, g.u32(at(c.edi, 0x1c))?);
    integer_to_ss(c, 1, c.eax)?;
    binary(c, 1, g.u32(SMALL)?, simd::subss)?;
    if above(c, ss(c, 1), g.u32(EPSILON)?)? {
        set_ss(c, 2, 0);
        if above(c, ss(c, 2), ss(c, 0))? {
            c.xmm[0] = c.xmm[2];
        } else if above(c, ss(c, 0), ss(c, 1))? {
            c.xmm[0] = c.xmm[1];
        }
        binary(c, 0, ss(c, 1), simd::divss)?;
        set_ss(c, 1, g.u32(ONE)?);
        g.set_u32(at(sp, 0x4c), ss(c, 0))?;
        if c.ebx & 0x40 != 0 {
            c.xmm[3] = c.xmm[1];
            binary(c, 3, ss(c, 0), simd::subss)?;
            g.set_u32(at(sp, 0x4c), ss(c, 3))?;
            c.xmm[0] = c.xmm[3];
        }
        if above(c, ss(c, 2), ss(c, 0))? {
            g.set_u32(at(sp, 0x4c), ss(c, 2))?;
        } else if above(c, ss(c, 0), ss(c, 1))? {
            g.set_u32(at(sp, 0x4c), ss(c, 1))?;
        }
    }
    Ok(())
}

/// Select the new action, optionally align its frame with the outgoing clip,
/// and install the primary and overlay channels. The callbacks can write the
/// selected references through the frame’s out-parameters.
fn install_transition(c: &mut CpuAbi, g: &mut Bus<'_>, mut pc: u32) -> sim_native::Result {
    loop {
        let sp = c.esp;
        pc = match pc {
            0x1cd070 => {
                c.esp = sp.wrapping_sub(0x38);
                c.edx = g.u32(at(c.esp, 0x40))?;
                push(c, g, c.ebx)?;
                push(c, g, c.ebp)?;
                c.ebp = g.u32(at(c.esp, 0x4c))?;
                push(c, g, c.esi)?;
                c.esi = c.eax;
                c.eax = g.u32(at(c.esp, 0x48))?;
                push(c, g, c.edi)?;
                c.edi = c.ecx;
                c.ecx = u32::MAX;
                let sp = c.esp;
                g.set_u32(at(sp, 0x28), c.eax)?;
                c.eax = g.u32(c.edi)?;
                let valid = c.eax != c.ecx;
                g.set_u8(at(sp, 0x54), 0)?;
                g.set_u16(at(sp, 0x18), c.ecx as u16)?;
                g.set_u16(at(sp, 0x1a), c.ecx as u16)?;
                g.set_u32(at(sp, 0x2c), c.edx)?;
                g.set_u32(at(sp, 0x30), c.ebp)?;
                g.set_u32(at(sp, 0x34), c.esi)?;
                g.set_u8(at(sp, 0x13), 1)?;
                if valid && g.u16(at(c.edi, 6))? != c.ecx as u16 {
                    c.ecx = g.u32(guest_layout::points::TAG_ENTRIES.base)?;
                    c.edx = g.u32(at(c.edi, 4))?;
                    c.eax = slot_offset(c, c.eax);
                    c.eax = g.u32(at(at(c.eax, c.ecx), 8))?;
                    push(c, g, c.edx)?;
                    c.edx = c.eax;
                    return g.call(c, 0x1daea0, 0x1cd0d7);
                }
                0x1cd0e4
            }
            0x1cd0d7 => {
                c.eax = byte(c.eax, !(g.u8(at(c.eax, 0x18))? >> 1) & 1);
                g.set_u8(at(sp, 0x13), c.eax as u8)?;
                0x1cd0e4
            }
            0x1cd0e4 => {
                c.ebx = g.u32(at(sp, 0x58))?;
                logic(c, c.ebx & 0x10, 0x80);
                if c.ebx & 0x10 != 0 {
                    c.eax = c.edi;
                    return g.call(c, 0x1ccba0, 0x1cd0f4);
                }
                0x1cd0f4
            }
            0x1cd0f4 => {
                c.ecx = g.u32(at(sp, 0x50))?;
                c.edx = g.u32(at(sp, 0x4c))?;
                mask_byte(c, g, at(c.edi, 0x6e), 0xfe, 0)?;
                c.eax = at(sp, 0x18);
                for v in [c.eax, c.ebx, c.esi, c.ebp, c.ecx, c.edx] {
                    push(c, g, v)?;
                }
                c.ecx = at(c.esp, 0x50);
                c.edx = at(c.esp, 0x40);
                c.eax = c.edi;
                return g.call(c, 0x1cc3f0, 0x1cd119);
            }
            0x1cd119 => {
                c.ebp = g.u32(at(sp, 0x34))?;
                if c.eax as u8 == 0 {
                    0x1cd4ce
                } else {
                    c.esi = u32::MAX;
                    if g.u16(at(sp, 0x1a))? == c.esi as u16 || c.ebx & 8 != 0 {
                        0x1cd4ce
                    } else {
                        logic(c, (c.ebx >> 8) & 2, 0x80);
                        g.set_u16(at(sp, 0x14), c.esi as u16)?;
                        g.set_u16(at(sp, 0x16), c.esi as u16)?;
                        g.set_u8(at(sp, 0x50), 0)?;
                        if c.ebx & 0x200 == 0 {
                            c.ecx = g.u32(at(sp, 0x28))?;
                            c.eax = at(sp, 0x50);
                            for v in [c.eax, c.ebp, c.ecx] {
                                push(c, g, v)?;
                            }
                            c.edx = at(c.esp, 0x58);
                            push(c, g, c.edx)?;
                            c.ebx = c.edi;
                            return g.call(c, 0x1ccda0, 0x1cd167);
                        }
                        0x1cd1c1
                    }
                }
            }
            0x1cd167 => {
                c.ecx = byte(c.ecx, g.u8(at(c.edi, 0x6c))?);
                c.eax = g.u32(c.eax)?;
                c.edx = byte(c.edx, g.u8(at(sp, 0x50))?);
                c.ecx = byte(c.ecx, !c.ecx as u8 & 1);
                g.set_u32(at(sp, 0x14), c.eax)?;
                c.eax = g.u32(c.edi)?;
                c.edx = byte(c.edx, c.edx as u8 & c.ecx as u8);
                g.set_u8(at(sp, 0x50), c.edx as u8)?;
                if c.eax != c.esi {
                    let clip = g.u16(at(c.edi, 6))?;
                    sub_flags(c, clip.into(), u32::from(c.esi as u16), 16);
                    if clip != c.esi as u16 {
                        c.eax = c.edi;
                        return g.call(c, 0xe6800, 0x1cd192);
                    }
                }
                c.eax = 0;
                0x1cd19f
            }
            0x1cd192 => {
                c.eax = u32::from(c.eax as u8 == 0);
                0x1cd19f
            }
            0x1cd19f => {
                c.edx = g.u32(at(sp, 0x58))?;
                c.ebx = g.u32(at(sp, 0x58))?;
                c.edx >>= 10;
                c.edx = byte(c.edx, c.edx as u8 & c.eax as u8);
                c.eax = byte(c.eax, g.u8(at(sp, 0x50))?);
                c.edx = byte(c.edx, c.edx as u8 & 1);
                c.eax = byte(c.eax, c.eax as u8 & c.edx as u8);
                g.set_u8(at(sp, 0x50), c.eax as u8)?;
                if c.eax as u8 != 0 {
                    mask_byte(c, g, at(c.edi, 0x6e), 0xff, 1)?;
                    0x1cd1c5
                } else {
                    0x1cd1c1
                }
            }
            0x1cd1c1 => {
                mask_byte(c, g, at(c.edi, 0x6e), 0xfe, 0)?;
                0x1cd1c5
            }
            0x1cd1c5 => {
                if g.u16(at(sp, 0x16))? == c.esi as u16 {
                    c.eax = byte(c.eax, g.u8(at(sp, 0x50))?);
                    if c.eax as u8 != 0 {
                        pc = 0x1cd4ce;
                        continue;
                    }
                }
                set_ss(c, 0, 0);
                c.eax = c.ebx & 0x20;
                g.set_u32(at(sp, 0x4c), ss(c, 0))?;
                g.set_u32(at(sp, 0x24), c.eax)?;
                if (c.eax != 0 || c.ebx & 0x40 != 0) && g.u32(c.edi)? != c.esi {
                    let clip = g.u16(at(c.edi, 6))?;
                    sub_flags(c, clip.into(), u32::from(c.esi as u16), 16);
                    if clip != c.esi as u16 {
                        c.ecx = c.edi;
                        return g.call(c, 0x1c6440, 0x1cd206);
                    }
                }
                0x1cd278
            }
            0x1cd206 => {
                playback_fraction(c, g)?;
                0x1cd278
            }
            0x1cd278 => {
                logic(c, (c.ebx >> 8) & 1, 0x80);
                g.set_u16(at(sp, 0x1c), c.esi as u16)?;
                g.set_u16(at(sp, 0x1e), c.esi as u16)?;
                g.set_u32(at(sp, 0x20), c.esi)?;
                if c.ebx & 0x100 == 0 {
                    c.edx = g.u32(at(sp, 0x30))?;
                    c.eax = g.u32(at(sp, 0x2c))?;
                    c.ecx = at(sp, 0x20);
                    push(c, g, c.ecx)?;
                    c.ecx = g.u32(at(c.esp, 0x2c))?;
                    for v in [c.edx, c.eax, c.ecx, c.edi] {
                        push(c, g, v)?;
                    }
                    c.ecx = c.ebp;
                    c.ebx = at(c.esp, 0x64);
                    return g.call(c, 0x1cccd0, 0x1cd2ab);
                }
                0x1cd2b1
            }
            0x1cd2ab => {
                c.edx = g.u32(c.eax)?;
                g.set_u32(at(sp, 0x1c), c.edx)?;
                0x1cd2b1
            }
            0x1cd2b1 => {
                c.eax = c.edi;
                return g.call(c, 0x1ccba0, 0x1cd2b8);
            }
            0x1cd2b8 => {
                c.eax = g.u32(at(sp, 0x44))?;
                c.ecx = g.u32(at(sp, 0x18))?;
                c.ebx = g.u32(at(sp, 0x5c))?;
                for v in [0, c.esi, c.esi, c.eax, c.ecx, c.edi] {
                    push(c, g, v)?;
                }
                c.eax = c.edi;
                return g.call(c, 0x1cb410, 0x1cd2d2);
            }
            0x1cd2d2 => {
                logic(c, u32::from(c.eax as u8), 0x80);
                if c.eax as u8 == 0 {
                    c.ebx = g.u32(at(sp, 0x58))?;
                    0x1cd4ce
                } else {
                    c.esi = c.edi;
                    g.set_u8(at(sp, 0x54), 1)?;
                    return g.call(c, 0x1c7300, 0x1cd2e6);
                }
            }
            0x1cd2e6 => {
                c.eax = byte(c.eax, c.eax as u8 & g.u8(at(sp, 0x13))?);
                g.set_u8(at(sp, 0x50), c.eax as u8)?;
                c.eax = u32::MAX;
                let clip = g.u16(at(sp, 0x16))?;
                sub_flags(c, clip.into(), u16::MAX.into(), 16);
                if clip != u16::MAX {
                    let graph = g.u32(at(c.edi, 0x68))?;
                    sub_flags(c, graph, c.eax, 32);
                    c.ebx = at(c.edi, 0x40);
                    if graph != c.eax {
                        c.edx = g.u32(at(sp, 0x14))?;
                        push(c, g, c.edx)?;
                        c.esi = at(c.esp, 0x60);
                        return g.call(c, 0x1cb3b0, 0x1cd310);
                    }
                }
                0x1cd354
            }
            0x1cd310 => {
                c.eax = g.u32(c.eax)?;
                c.ecx = g.u32(at(c.edi, 0x68))?;
                for v in [3, u32::MAX, u32::MAX, c.ebp, c.eax] {
                    push(c, g, v)?;
                }
                c.edx = g.u32(0x1cd31e)?;
                c.eax = c.ebx;
                return g.call(c, 0x1c6470, 0x1cd329);
            }
            0x1cd329 => {
                logic(c, u32::from(c.eax as u8), 0x80);
                if c.eax as u8 == 0 {
                    0x1cd354
                } else {
                    push(c, g, 0)?;
                    c.eax = c.ebx;
                    return g.call(c, 0x1c6560, 0x1cd336);
                }
            }
            0x1cd336 => {
                let flag = g.u8(at(c.edi, 0x12))? & 1;
                logic(c, flag.into(), 0x80);
                if flag != 0 {
                    mask_byte(c, g, at(c.edi, 0x11), 0xff, 1)?;
                }
                c.esi = c.ebx;
                return g.call(c, 0x1c7300, 0x1cd347);
            }
            0x1cd347 => {
                let flag = g.u8(at(sp, 0x50))? & c.eax as u8;
                g.set_u8(at(sp, 0x50), flag)?;
                logic(c, flag.into(), 0x80);
                if flag != 0 {
                    c.ebx = c.edi;
                    return g.call(c, 0x1ce010, 0x1cd354);
                }
                0x1cd354
            }
            0x1cd354 => {
                c.eax = at(c.edi, 0x40);
                return g.call(c, 0xb6760, 0x1cd35c);
            }
            0x1cd35c => {
                if c.eax as u8 != 0 {
                    0x1cd3a8
                } else {
                    c.eax = g.u32(at(sp, 0x24))?;
                    if c.eax != 0 || g.u8(at(sp, 0x58))? & 0x40 != 0 {
                        set_ss(c, 0, g.u32(at(sp, 0x4c))?);
                        if above(c, ss(c, 0), g.u32(EPSILON)?)? {
                            c.ecx = c.edi;
                            return g.call(c, 0x1c6440, 0x1cd385);
                        }
                    }
                    0x1cd3a8
                }
            }
            0x1cd385 => {
                c.eax = g.u16(at(c.eax, 0x14))? as i16 as i32 as u32;
                integer_to_ss(c, 0, c.eax)?;
                binary(c, 0, g.u32(SMALL)?, simd::subss)?;
                binary(c, 0, g.u32(at(sp, 0x4c))?, simd::mulss)?;
                push(c, g, c.ecx)?;
                c.eax = c.edi;
                g.set_u32(c.esp, ss(c, 0))?;
                return g.call(c, 0x1c6560, 0x1cd3a8);
            }
            0x1cd3a8 => {
                c.eax = byte(c.eax, g.u8(at(sp, 0x50))?);
                if c.eax as u8 != 0 {
                    mask_byte(c, g, at(c.edi, 0x6c), 0xef, 0)?;
                } else {
                    mask_byte(c, g, at(c.edi, 0x6c), 0xff, 0x10)?;
                }
                let clip = g.u16(at(sp, 0x1e))?;
                sub_flags(c, clip.into(), u16::MAX.into(), 16);
                if clip != u16::MAX {
                    c.ecx = g.u32(at(c.edi, 0x68))?;
                    sub_flags(c, c.ecx, u32::MAX, 32);
                    if c.ecx != u32::MAX {
                        c.edx = g.u32(at(sp, 0x20))?;
                        c.eax = g.u32(at(sp, 0x1c))?;
                        for v in [0, u32::MAX, u32::MAX, c.edx, c.eax] {
                            push(c, g, v)?;
                        }
                        c.eax = at(c.edi, 0x20);
                        c.edx = g.u32(0x1cd3de)?;
                        return g.call(c, 0x1c6470, 0x1cd3e7);
                    }
                }
                0x1cd3f6
            }
            0x1cd3e7 => {
                if c.eax as u8 != 0 {
                    set_ss(c, 0, 0);
                    g.set_u32(at(c.edi, 0x80), 0)?;
                }
                0x1cd3f6
            }
            0x1cd3f6 => {
                mask_byte(c, g, at(c.edi, 0x6c), 0xfe, 0)?;
                0x1cd3fa
            }
            0x1cd3fa => {
                c.ebp = g.u32(at(c.edi, 0x70))?;
                c.esi = g.u32(at(sp, 0x28))?;
                c.ebx = g.u32(at(c.edi, 0x74))?;
                c.edx = g.u32(at(sp, 0x2c))?;
                let mode_changed = c.ebp != c.esi;
                c.ebp = g.u32(at(c.edi, 0x78))?;
                c.eax = at(c.edi, 0x70);
                c.ecx = byte(c.ecx, u8::from(mode_changed));
                let class_changed = c.ebx != c.edx;
                c.ebx = g.u32(at(sp, 0x30))?;
                c.edx = byte(c.edx, u8::from(class_changed));
                let kind_changed = c.ebp != c.ebx;
                c.ebp = c.eax;
                g.set_u32(c.ebp, c.esi)?;
                c.esi = g.u32(at(sp, 0x2c))?;
                g.set_u32(at(c.ebp, 4), c.esi)?;
                c.esi = g.u32(at(sp, 0x30))?;
                g.set_u32(at(c.ebp, 8), c.esi)?;
                c.esi = g.u32(at(sp, 0x34))?;
                c.ebx = byte(c.ebx, u8::from(kind_changed));
                g.set_u32(at(c.ebp, 0xc), c.esi)?;
                if mode_changed || class_changed || kind_changed {
                    c.ecx = g.u32(at(c.edi, 0x68))?;
                    c.esi = u32::MAX;
                    if c.ecx != c.esi {
                        let flag = g.u8(at(c.edi, 0x6c))? & 1;
                        logic(c, flag.into(), 0x80);
                        if flag == 0 {
                            c.eax = g.u32(c.eax)?;
                            c.edx = g.u32(at(c.edi, 0x78))?;
                            c.ecx = g.u32(at(c.edi, 0x74))?;
                            for v in [1, 1, c.eax] {
                                push(c, g, v)?;
                            }
                            return g.call(c, 0x1ce180, 0x1cd469);
                        }
                    }
                    0x1cd469
                } else {
                    0x1cd4bc
                }
            }
            0x1cd469 => {
                if g.u32(at(c.edi, 0x40))? != c.esi && g.u16(at(c.edi, 0x46))? != c.esi as u16 {
                    0x1cd4bc
                } else {
                    c.eax = byte(c.eax, 0xfe);
                    disable_channel(c, g, at(c.edi, 0x40))?;
                    if g.u32(at(c.edi, 0x40))? == c.esi || g.u16(at(c.edi, 0x46))? == c.esi as u16 {
                        disable_channel(c, g, c.edi)?;
                        disable_channel(c, g, at(c.edi, 0x20))?;
                    }
                    0x1cd4bc
                }
            }
            0x1cd4ce => {
                if c.ebx & 4 != 0 {
                    g.set_u8(at(sp, 0x54), 1)?;
                    0x1cd3fa
                } else {
                    0x1cd4bc
                }
            }
            0x1cd4bc => {
                c.eax = byte(c.eax, g.u8(at(sp, 0x54))?);
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                c.ebx = pop(c, g)?;
                c.esp = arithmetic(c, c.esp, 0x38, false);
                return g.ret(c, 0x14);
            }
            _ => return Err(bad(pc)),
        };
    }
}

fn disable_channel(c: &mut CpuAbi, g: &mut Bus<'_>, channel: u32) -> Checked<()> {
    if g.u32(channel)? != c.esi
        && g.u16(at(channel, 6))? != c.esi as u16
        && g.u8(at(channel, 0x12))? & 1 != 0
    {
        mask_byte(c, g, at(channel, 0x11), c.eax as u8, 0)?;
    }
    Ok(())
}
